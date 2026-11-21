//! The floor loop (A3), run between batches off the step's thread: the
//! residual frames the step handed over become per-line spectra, averaged
//! with the tones' and servoed bands' lines excluded; the fill rule covers
//! the excluded lines; MIMO Random's checks, caps, ceilings and limits take
//! the floor law's answer; the servoed bands' laws run in their own frames;
//! the drive in force is synthesized ahead into the step's ring.

use super::*;
use crate::band::{increment_stack, mean_diagonal, place_band_drive, weights_at, BandLoopInputs};
use crate::exclusion::{band_span_over_frame, frame_mask_default_guard, ToneSpan};
use crate::fill::FillInput;
use hertzforge_mimo_random::guards;
use hertzforge_mimo_random::law::{guarded_call, Miss};
use hertzforge_mimo_random::pool;
use hertzforge_random::service::engine::DriveFactor;
use hertzforge_sdm::law::{MimoLoopInputs, MimoLoopOutputs};

/// Loops in a row a servoed band's mean level may sit outside its abort band.
pub const BAND_ABORT_LOOPS: u32 = 3;

/// What one batch's end looked like: the input frame it ended at, each
/// tone's frequency (`NaN` when out) and each band's center (`NaN` when
/// out).
pub(crate) type Snapshot = (u64, Vec<f64>, Vec<f64>);

impl MimoMixedService {
    /// Frames the ring is kept ahead by.
    fn ring_target(&self) -> usize {
        let block = self.actual.map_or(256, |a| a.block_frames.max(1));
        (CHUNK_S * self.fs) as usize + 2 * TICKS_PER_BATCH as usize * block
    }

    /// Fresh floor-loop state for the run.
    pub(crate) fn reset_loop(&mut self) -> Result<(), MimoMixedError> {
        let q = self.cfg.n() + self.cfg.p();
        self.spectra = Some(FrameSpectra::new(
            self.nfft,
            self.fs,
            q,
            self.lines,
            self.cfg.estimation.window,
        ));
        self.est = Some(MatrixFloorEstimator::for_estimation(
            q,
            self.lines,
            &self.cfg.estimation,
        ));
        self.bufs = vec![VecDeque::with_capacity(4 * self.nfft); q];
        self.gains.clear();
        self.resp_at = 0;
        self.gain_at = 0;
        self.snapshots.clear();
        self.frames_in_loop = 0;
        self.loops = 0;
        if self.fill.is_none() {
            self.fill = Some(match self.fill_override.take() {
                Some(r) => r,
                None => crate::fill::fill_rule(&self.cfg.law.fill_rule).map_err(|_| {
                    MimoMixedConfigError::UnknownFillRule {
                        id: self.cfg.law.fill_rule.id.clone(),
                    }
                })?,
            });
        }
        self.refactor = true;
        Ok(())
    }

    /// The synthesized drive spectral matrix: the floor's plus every servoed
    /// band's placed drive under its ramp, full level.
    fn total_drive(&mut self) -> Option<HermitianStack> {
        let mut s = self.drive_full.clone()?;
        let m = self.cfg.m();
        let lines = self.lines;
        let df = self.df;
        for b in &mut self.bands {
            let (Some(d), Some(mfe)) = (b.drive.as_ref(), b.mfe.as_ref()) else {
                continue;
            };
            if b.ramp <= 0.0 {
                continue;
            }
            let center = b.center_hz + b.rate_hz_s * b.ahead_s;
            let mut w = vec![0.0; lines];
            weights_at(&b.width, center, df, &mut w);
            let wo: Vec<f64> = (0..mfe.offsets())
                .map(|o| mfe.line_of(o, center, df, lines).map_or(0.0, |l| w[l]))
                .collect();
            let out = crate::band::BandLoopOutput {
                drive: d.clone(),
                declared: crate::band::DriveDeclared::Increment,
                capped: false,
            };
            let Ok(placed) = place_band_drive(&out, &wo) else {
                continue;
            };
            let r2 = b.ramp * b.ramp;
            for o in 0..mfe.offsets() {
                let Some(l) = mfe.line_of(o, center, df, lines) else {
                    continue;
                };
                for r in 0..m {
                    for c in r..m {
                        let v = s.get(l, r, c) + placed.get(o, r, c) * r2;
                        s.set(l, r, c, v);
                    }
                }
            }
            b.drawn_hz = center;
        }
        Some(s)
    }

    /// Rebuild the drive factor from the drive in force.
    fn build_factor(&mut self) -> Result<(), MimoMixedError> {
        let Some(s) = self.total_drive() else {
            return Ok(());
        };
        let m = self.cfg.m();
        let l = pool::sqrt_factor(&s, self.band)
            .map_err(|e| MimoMixedError::Law(format!("the drive's factor: {e}")))?;
        let mut f = DriveFactor::zeros(self.lines, m, m);
        for line in self.band.0..self.band.1 {
            let src = l.line(line);
            let dst = &mut f.data[line * m * m..(line + 1) * m * m];
            for (d, v) in dst.iter_mut().zip(src) {
                *d = (v.re, v.im);
            }
        }
        self.clip_v = match self.cfg.drive_clip_sigma {
            Some(sig) if sig > 0.0 => guards::drive_rms(&s, self.band, self.df)
                .iter()
                .map(|r| sig * r)
                .collect(),
            _ => vec![f64::INFINITY; m],
        };
        self.factor = Some(f);
        self.refactor = false;
        Ok(())
    }

    /// Keep the step's ring ahead with the drive in force (whole hops, so the
    /// synthesis stays seamless), each drive clipped at its sigma clip.
    pub(crate) fn top_up(&mut self) -> Result<(), MimoMixedError> {
        if self.step.is_none() || self.synth.is_none() {
            return Ok(());
        }
        let ahead = self.step.as_ref().map_or(0, |s| s.ring.frames());
        let ahead_s = ahead as f64 / self.fs;
        let mut moved = false;
        for b in &mut self.bands {
            b.ahead_s = ahead_s;
            if b.drive.is_some()
                && b.ramp > 0.0
                && ((b.center_hz + b.rate_hz_s * ahead_s) - b.drawn_hz).abs() > 0.5 * self.df
            {
                moved = true;
            }
        }
        if self.refactor || moved || self.factor.is_none() {
            self.build_factor()?;
        }
        let Some(factor) = self.factor.as_ref() else {
            return Ok(());
        };
        let m = self.cfg.m();
        let hop = (self.nfft / 2).max(1);
        let target = self.ring_target();
        let (have, space) = self
            .step
            .as_ref()
            .map_or((0, 0), |s| (s.ring.frames(), s.ring.space()));
        if have >= target {
            return Ok(());
        }
        let want = ((target - have).div_ceil(hop) * hop).min(space / hop * hop);
        if want == 0 {
            return Ok(());
        }
        let synth = self.synth.as_mut().expect("synth");
        let mut v = synth
            .frames_factored(want, factor)
            .map_err(MimoMixedError::Law)?;
        for (i, x) in v.iter_mut().enumerate() {
            let c = self.clip_v.get(i % m).copied().unwrap_or(f64::INFINITY);
            *x = x.clamp(-c, c);
        }
        if let Some(s) = self.step.as_mut() {
            s.ring.push(&v);
        }
        Ok(())
    }

    /// Take what the step handed over: the tone laws' records out, the
    /// residual frames into the floor loop (or the identification's
    /// record). Returns the frames taken.
    pub(crate) fn take_records(&mut self) -> usize {
        let Some(step) = self.step.as_mut() else {
            return 0;
        };
        let frames = step.records.frames();
        let q = self.cfg.n() + self.cfg.p();
        let m = self.cfg.m();
        // Each drive's rms and peak over the batch.
        let mut rms = vec![0.0; m];
        let mut peak = vec![0.0f64; m];
        for f in 0..frames {
            for j in 0..m {
                let d = step.records.drive[f * m + j];
                rms[j] += d * d;
                peak[j] = peak[j].max(d.abs());
            }
        }
        self.chunk_rms = rms
            .iter()
            .map(|s| (s / frames.max(1) as f64).sqrt())
            .collect();
        self.chunk_peak = peak;
        if let Some(rec) = self.ident_rec.as_mut() {
            for f in 0..frames {
                for c in 0..q {
                    rec.resp[c].push(step.records.raw[f * q + c]);
                }
                for j in 0..m {
                    rec.drive[j].push(step.records.drive[f * m + j]);
                }
            }
        } else if self.bufs.len() == q {
            for f in 0..frames {
                for c in 0..q {
                    self.bufs[c].push_back(step.records.residual[f * q + c]);
                }
                self.gains.push_back(step.records.gain[f]);
            }
        }
        step.records.clear();
        // A copy, so the step's preallocated buffer keeps its room.
        let law: Vec<LawRecord> = step.records.law.clone();
        step.records.law.clear();
        // The batch's snapshot: tones and bands where they are.
        let now = step.clock.now();
        let tones: Vec<f64> = (0..step.tones.len())
            .map(|k| {
                if step.bank.part_level(k) > 0.0 || now[step.tones[k].part].on {
                    step.bank.frequency(k)
                } else {
                    f64::NAN
                }
            })
            .collect();
        let live = step.parts_live;
        let dt = frames as f64 / self.fs;
        let ramp_step = if self.cfg.part_ramp_s > 0.0 {
            dt / self.cfg.part_ramp_s
        } else {
            1.0
        };
        let ending = self.ending.is_some();
        let mut bands = Vec::with_capacity(self.bands.len());
        let mut refactor = false;
        for b in &mut self.bands {
            let pn = &now[b.part];
            let c = pn.freq_hz.unwrap_or(b.center_hz);
            if dt > 0.0 {
                b.rate_hz_s = (c - b.center_hz) / dt;
            }
            b.center_hz = c;
            if b.law.is_some() {
                let on = live && pn.on && !ending;
                let was = b.ramp;
                b.ramp = if on {
                    (b.ramp + ramp_step).min(1.0)
                } else {
                    (b.ramp - ramp_step).max(0.0)
                };
                if b.ramp != was {
                    refactor = true;
                }
                bands.push(if b.ramp > 0.0 || on { c } else { f64::NAN });
            } else {
                b.ramp = 1.0;
                bands.push(f64::NAN);
            }
        }
        if refactor {
            self.refactor = true;
        }
        self.push_law_records(&law);
        if self.ident_rec.is_none() && self.bufs.len() == q {
            let end = self.resp_at + self.bufs.first().map_or(0, |b| b.len()) as u64;
            self.snapshots.push_back((end, tones, bands));
            self.floor_frames();
        }
        frames
    }

    /// Analyse every whole frame the buffers hold.
    fn floor_frames(&mut self) {
        let nfft = self.nfft;
        let hop = (nfft / 2).max(1);
        let lag = self.ident.as_ref().map_or(0, |i| i.lag_frames) as u64;
        let q = self.cfg.n() + self.cfg.p();
        let n = self.cfg.n();
        let half_hz = self.cfg.exclusion_hz / 2.0;
        let mut mask = vec![false; self.lines];
        let mut tone_mask = vec![false; self.lines];
        let mut scaled = vec![C64::new(0.0, 0.0); self.lines * q];
        let mut ctl = vec![C64::new(0.0, 0.0); self.lines * n];
        while self.bufs.first().is_some_and(|b| b.len() >= nfft)
            && self.state == MimoMixedState::Running
        {
            let a = self.resp_at;
            let b_end = a + nfft as u64;
            // The random gain at drive time.
            let steady = if a >= lag {
                let g0 = a - lag;
                let skip = g0.saturating_sub(self.gain_at) as usize;
                let g: Vec<f64> = self.gains.iter().skip(skip).take(nfft).copied().collect();
                if g.len() == nfft {
                    let mean = g.iter().sum::<f64>() / nfft as f64;
                    let (lo, hi) = g
                        .iter()
                        .fold((f64::INFINITY, 0.0f64), |(l, h), &x| (l.min(x), h.max(x)));
                    (mean > 0.0 && (hi - lo) <= GAIN_STEADY * mean).then_some(mean)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(gain) = steady {
                for buf in &mut self.bufs {
                    buf.make_contiguous();
                }
                let spectra = self.spectra.as_mut().expect("spectra");
                let bufs = &self.bufs;
                spectra.transform(|c| bufs[c].as_slices().0);
                let inv = 1.0 / gain;
                for (d, s) in scaled.iter_mut().zip(spectra.spectra()) {
                    *d = *s * inv;
                }
                // The mask: every tone in and every servoed band, over the
                // frame (at drive time).
                let lo = a.saturating_sub(lag);
                let snaps: Vec<&Snapshot> = self
                    .snapshots
                    .iter()
                    .filter(|s| s.0 >= lo && s.0 <= b_end + hop as u64)
                    .collect();
                let mut spans = Vec::new();
                let mut bspans = Vec::new();
                if let Some(first) = snaps.first() {
                    for k in 0..first.1.len() {
                        let fs: Vec<f64> = snaps
                            .iter()
                            .map(|s| s.1[k])
                            .filter(|f| f.is_finite())
                            .collect();
                        if let (Some(f0), Some(f1)) = (fs.first(), fs.last()) {
                            spans.push(ToneSpan {
                                start_hz: *f0,
                                end_hz: *f1,
                            });
                        }
                    }
                    for (i, bc) in self.bands.iter().enumerate() {
                        if bc.law.is_none() {
                            continue;
                        }
                        let cs: Vec<f64> = snaps
                            .iter()
                            .map(|s| s.2[i])
                            .filter(|f| f.is_finite())
                            .collect();
                        if let (Some(c0), Some(c1)) = (cs.first(), cs.last()) {
                            bspans.push(band_span_over_frame(&bc.width, *c0, *c1));
                        }
                    }
                }
                frame_mask_default_guard(self.df, &spans, half_hz, &bspans, &mut mask);
                frame_mask_default_guard(self.df, &spans, half_hz, &[], &mut tone_mask);
                if let Some(est) = self.est.as_mut() {
                    est.add_frame(&scaled, &mask);
                }
                if self.bands.iter().any(|b| b.mfe.is_some()) {
                    for l in 0..self.lines {
                        ctl[l * n..(l + 1) * n].copy_from_slice(&scaled[l * q..l * q + n]);
                    }
                    let mid = snaps.get(snaps.len() / 2).map(|s| s.2.clone());
                    for (i, bc) in self.bands.iter_mut().enumerate() {
                        let center = mid.as_ref().map_or(bc.center_hz, |v| v[i]);
                        if let (Some(mfe), true) = (bc.mfe.as_mut(), center.is_finite()) {
                            mfe.add_frame(&ctl, center, self.df, &tone_mask);
                        }
                    }
                }
                self.frames_in_loop += 1;
            } else {
                self.unsteady += 1;
            }
            // Advance a hop.
            for buf in &mut self.bufs {
                buf.drain(..hop);
            }
            self.resp_at += hop as u64;
            let keep_from = self.resp_at.saturating_sub(lag);
            while self.gain_at < keep_from && !self.gains.is_empty() {
                self.gains.pop_front();
                self.gain_at += 1;
            }
            let oldest = self.resp_at.saturating_sub(lag + 2 * nfft as u64);
            while self.snapshots.len() > 2 && self.snapshots.front().is_some_and(|s| s.0 < oldest) {
                self.snapshots.pop_front();
            }
            if self.frames_in_loop >= self.cfg.estimation.frames_per_loop.max(1) {
                self.frames_in_loop = 0;
                self.floor_loop();
                if self.ending.is_some() {
                    break;
                }
            }
        }
    }

    /// One floor loop: the estimate, the fill, the judging, the law, the
    /// bands.
    fn floor_loop(&mut self) {
        let (n, p) = (self.cfg.n(), self.cfg.p());
        let band = self.band;
        let lines = self.lines;
        let Some(est) = self.est.as_mut() else { return };
        est.end_loop();
        let dof = est.min_clean_dof(band);
        let clean = est.clean_mask();
        // The lines judged: clean lines whose own estimate holds
        // `MIN_ENVELOPE_DOF`. A line a moving part has just left is clean
        // again but young; judging only on the band's least degrees of
        // freedom would leave the floor unjudged for as long as a part moves.
        let ripe: Vec<bool> = (0..lines)
            .map(|l| clean[l] && est.dof(l) >= MIN_ENVELOPE_DOF)
            .collect();
        let mut meas = HermitianStack::zeros(lines, n);
        let mut lim = vec![0.0; lines * p];
        for l in 0..lines {
            for r in 0..n {
                for c in r..n {
                    meas.set(l, r, c, est.get(l, r, c));
                }
            }
            for i in 0..p {
                lim[l * p + i] = est.get(l, n + i, n + i).re;
            }
        }
        let Some(floor_ref) = self.floor_ref.clone() else {
            return;
        };
        let (Some(model), Some(drive)) = (self.model.clone(), self.drive_full.clone()) else {
            return;
        };
        // The fill.
        let mut filled = meas.clone();
        if let Some(fill) = self.fill.as_mut() {
            let rep = fill.fill(
                &FillInput {
                    measured: &meas,
                    clean: &clean,
                    reference: &floor_ref,
                    model: &model,
                    drive: &drive,
                    band,
                },
                &mut filled,
            );
            self.fill_report = Some(rep);
        }
        // The floor per control on clean lines.
        self.clean_lines = (band.0..band.1).filter(|&l| clean[l]).count();
        let mut err = vec![f64::NAN; n];
        for (i, e) in err.iter_mut().enumerate() {
            let (mut a, mut r) = (0.0, 0.0);
            for l in (band.0..band.1).filter(|&l| ripe[l]) {
                a += meas.get(l, i, i).re.max(0.0);
                r += floor_ref.get(l, i, i).re.max(0.0);
            }
            if a > 0.0 && r > 0.0 {
                *e = 10.0 * (a / r).log10();
            }
        }
        self.floor_err_db = err.clone();
        let judged = (band.0..band.1).any(|l| ripe[l]);
        self.equalized = judged && err.iter().all(|e| e.abs() <= FLOOR_EQUALIZED_DB);
        if judged && !self.judge_floor(&meas, &ripe, &lim) {
            return;
        }
        if !self.judge_folded_bands(&meas, &floor_ref, &ripe) {
            return;
        }
        // The floor law.
        self.loops += 1;
        let t0 = std::time::Instant::now();
        let before = self.drive_sum_rms();
        let id = self.ident.take();
        let r = match id.as_ref() {
            Some(id) => self.call_floor(self.loops, &filled, dof, &model, id),
            None => Err(Miss {
                reason: hertzforge_mimo_random::report::MimoFaultReason::Error,
                detail: "no identification".into(),
            }),
        };
        self.ident = id;
        let mut miss = 0u8;
        if let Err(words) = r.map_err(|m| m.words()).and_then(|o| self.accept_floor(o)) {
            miss = 1;
            self.floor_misses += 1;
            self.note(format!("Floor law miss: {words}"));
            let max = hertzforge_random::service::config::LawGuards::default().max_misses;
            if self.floor_misses >= max {
                self.abort_with(RunEnd {
                    name: "law_fault",
                    aborted: true,
                    part: self.floor_id(),
                    control: None,
                    drive: None,
                    words: format!(
                        "the floor law missed {} loops in a row ({words}); the controlled stop",
                        self.floor_misses
                    ),
                });
            }
        }
        let rec = LawRecord {
            t_s: self.t_s,
            drive_in: before,
            drive_out: self.drive_sum_rms(),
            took_us: (t0.elapsed().as_secs_f64() * 1e6) as f32,
            role: records::RecordRole::Floor,
            tone: 0,
            miss,
            flags: 0,
        };
        self.push_law_records(&[rec]);
        if self.ending.is_none() {
            self.band_loops(&model, &drive);
        }
    }

    /// Each folded band on its mean added level, as a servoed band is read:
    /// over the lines the band covers whole, the floor estimate less the
    /// floor's own reference against the band's increment (the floor law's
    /// reference less the floor's), dB per control, on the judged (`ripe`)
    /// lines; NaN while none of its lines is judged. Outside its abort band for `BAND_ABORT_LOOPS` loops in a
    /// row it aborts `band_abort`, naming the band and the control. `false`
    /// when the run is ending.
    fn judge_folded_bands(
        &mut self,
        meas: &HermitianStack,
        floor_ref: &HermitianStack,
        ripe: &[bool],
    ) -> bool {
        let n = self.cfg.n();
        let df = self.df;
        let mut trip: Option<RunEnd> = None;
        let mut w = vec![0.0; self.lines];
        for b in self.bands.iter_mut().filter(|b| b.law.is_none()) {
            crate::band::weights_at(&b.width, b.center_hz, df, &mut w);
            for i in 0..n {
                let (mut a, mut st) = (0.0, 0.0);
                for (l, _) in w.iter().enumerate().filter(|&(l, &x)| x >= 1.0 && ripe[l]) {
                    let f = self.cfg.reference.sdm.get(l, i, i).re;
                    a += meas.get(l, i, i).re - f;
                    st += floor_ref.get(l, i, i).re - f;
                }
                b.error_db[i] = if st <= 0.0 {
                    f64::NAN
                } else if a > 0.0 {
                    10.0 * (a / st).log10()
                } else {
                    -99.0
                };
                let e = b.error_db[i];
                let (lo, hi) = b.abort_db[i];
                if e.is_finite() && (e < -lo || e > hi) {
                    b.over_loops[i] += 1;
                } else {
                    b.over_loops[i] = 0;
                }
                if b.over_loops[i] >= BAND_ABORT_LOOPS {
                    trip.get_or_insert(RunEnd {
                        name: "band_abort",
                        aborted: true,
                        part: Some(b.id.clone()),
                        control: Some(i),
                        drive: None,
                        words: format!(
                            "band {} read {e:.2} dB on control {i}, outside its abort band",
                            b.id
                        ),
                    });
                }
            }
        }
        match trip {
            Some(t) => {
                self.abort_with(t);
                false
            }
            None => true,
        }
    }

    fn drive_sum_rms(&self) -> f64 {
        self.drive_full.as_ref().map_or(0.0, |d| {
            guards::drive_rms(d, self.band, self.df).iter().sum()
        })
    }

    /// The floor's envelope per control on clean lines (relaxed while the
    /// floor plays alone at the soft start) and each limit's abort. `false`
    /// when the run is ending.
    fn judge_floor(&mut self, meas: &HermitianStack, clean: &[bool], lim: &[f64]) -> bool {
        let n = self.cfg.n();
        let p = self.cfg.p();
        let tol = self.floor_tolerances();
        let relax_db = if self.phase == MimoMixedPhase::FloorAlone {
            self.cfg.soft_start_relax_db
        } else {
            0.0
        };
        let relax = 10f64.powf(relax_db / 10.0);
        for i in 0..n {
            let mut abort = 0usize;
            for l in (self.band.0..self.band.1).filter(|&l| clean[l]) {
                let k = l * n + i;
                let v = meas.get(l, i, i).re;
                let hi = tol.abort_upper.get(k).copied().unwrap_or(f64::NAN);
                let lo = tol.abort_lower.get(k).copied().unwrap_or(f64::NAN);
                if (!hi.is_nan() && v > hi * relax) || (!lo.is_nan() && v < lo / relax) {
                    abort += 1;
                }
            }
            if abort >= self.cfg.protections.envelope_abort_lines {
                self.abort_with(RunEnd {
                    name: "floor_envelope",
                    aborted: true,
                    part: self.floor_id(),
                    control: Some(i),
                    drive: None,
                    words: format!(
                        "the floor on control {i} was outside its abort lines on {abort} clean \
                         lines"
                    ),
                });
                return false;
            }
        }
        // The limits, at the level in force.
        let lam = self.step.as_ref().map_or(0.0, |s| s.level.value());
        for i in 0..p {
            let spec = &self.cfg.limits[i];
            let over = 10f64.powf(spec.abort_margin_db / 10.0);
            let mut worst = 0usize;
            for l in self.band.0..self.band.1 {
                let c = spec.ceiling(l);
                if c > 0.0 && lim[l * p + i] * lam * lam > c * over {
                    worst += 1;
                }
            }
            if worst >= self.cfg.protections.envelope_abort_lines {
                let words = format!(
                    "limit channel {} was over its ceiling plus {:.1} dB on {worst} lines",
                    spec.channel_index, spec.abort_margin_db
                );
                self.abort_with(RunEnd {
                    name: "limit_abort",
                    aborted: true,
                    part: self.floor_id(),
                    control: None,
                    drive: None,
                    words,
                });
                return false;
            }
        }
        true
    }

    /// One call of the floor law (the seed at loop 0), under MIMO Random's
    /// guarded call and time budget.
    pub(crate) fn call_floor(
        &mut self,
        loop_index: u64,
        resp: &HermitianStack,
        dof: f64,
        model: &MatrixStack,
        id: &MimoMixedIdent,
    ) -> Result<MimoLoopOutputs, Miss> {
        let want = |x: MimoInput| self.floor_needs.contains(&x);
        let last_drive = if want(MimoInput::LastDrive) {
            self.drive_full.clone()
        } else {
            None
        };
        let sysid_response = want(MimoInput::SysidResponse).then(|| id.g_cc.clone());
        let sysid_drive = want(MimoInput::SysidDrive).then(|| id.g_dd.clone());
        let reference = self
            .floor_ref
            .clone()
            .unwrap_or_else(|| self.loop_reference());
        let tolerances = self.cfg.floor_tolerances_about(&reference);
        let inputs = MimoLoopInputs {
            reference: &reference,
            tolerances: &tolerances,
            response: resp,
            response_dof: dof,
            frf: model,
            multiple_coherence: &id.coherence,
            df_hz: self.df,
            band: self.band,
            loop_index,
            last_drive: last_drive.as_ref(),
            response_drive_csd: None,
            sysid_response: sysid_response.as_ref(),
            sysid_drive: sysid_drive.as_ref(),
            sysid_noise: None,
        };
        let budget = self.floor_budget_s();
        let clock = self.floor_clock.clone();
        let Some(law) = self.floor_law.as_mut() else {
            return Err(Miss {
                reason: hertzforge_mimo_random::report::MimoFaultReason::Error,
                detail: "no floor law".into(),
            });
        };
        guarded_call(law.as_mut(), clock.as_ref(), budget, |l| {
            if loop_index == 0 {
                l.seed(&inputs)
            } else {
                l.iterate(&inputs)
            }
        })
    }

    /// The floor law's answer becomes the drive in force: checked, capped
    /// against the drive in force, held under each drive's ceiling, the
    /// limits applied (MIMO Random's order).
    pub(crate) fn accept_floor(&mut self, out: MimoLoopOutputs) -> Result<(), String> {
        let m = self.cfg.m();
        let mut d = out.drive;
        guards::check_output(&mut d, self.lines, m, self.band).map_err(|(_, s)| s)?;
        if let Some(old) = self.drive_full.as_ref() {
            guards::cap_change(
                &mut d,
                old,
                self.band,
                self.cfg.caps.up_db,
                self.cfg.caps.down_db,
            );
        }
        let ceilings: Vec<f64> = self.cfg.drives.iter().map(|x| x.ceiling_vrms).collect();
        guards::apply_ceilings(&mut d, self.band, self.df, &ceilings);
        if let Some(h_l) = self.h_l.as_ref().filter(|_| self.cfg.p() > 0) {
            let limits = self.cfg.limits.clone();
            guards::limit_responses(&mut d, h_l, &|i, line| limits[i].ceiling(line), self.band);
        }
        self.drive_full = Some(d);
        self.floor_misses = 0;
        self.refactor = true;
        Ok(())
    }

    /// Each servoed band's law in its own frame; its level judged on its
    /// mean over the floor.
    fn band_loops(&mut self, model: &MatrixStack, drive: &HermitianStack) {
        let (n, m) = (self.cfg.n(), self.cfg.m());
        let lines = self.lines;
        let df = self.df;
        let mut trip: Option<RunEnd> = None;
        let mut any = false;
        for b in &mut self.bands {
            let (Some(law), Some(setup), Some(mfe)) =
                (b.law.as_mut(), b.setup.as_ref(), b.mfe.as_ref())
            else {
                continue;
            };
            if b.ramp <= 0.0 {
                continue;
            }
            any = true;
            let center = b.center_hz;
            let offs = mfe.offsets();
            let mut fref = HermitianStack::zeros(offs, n);
            let mut fdrv = HermitianStack::zeros(offs, m);
            let mut z = MatrixStack::zeros(offs, n, m);
            for o in 0..offs {
                let Some(l) = mfe.line_of(o, center, df, lines) else {
                    continue;
                };
                for r in 0..n {
                    for c in r..n {
                        fref.set(o, r, c, self.cfg.reference.sdm.get(l, r, c));
                    }
                    for c in 0..m {
                        z.set(o, r, c, model.get(l, r, c));
                    }
                }
                for r in 0..m {
                    for c in r..m {
                        fdrv.set(o, r, c, drive.get(l, r, c));
                    }
                }
            }
            let inc = increment_stack(&b.statement, &fref);
            let estimate = mfe.estimate();
            let bdof = mfe.min_dof(setup.inner);
            let band_drive = b
                .drive
                .clone()
                .unwrap_or_else(|| HermitianStack::zeros(offs, m));
            let inputs = BandLoopInputs {
                floor_reference: &fref,
                increment: &inc,
                estimate: &estimate,
                dof: bdof,
                model_next: &z,
                floor_drive: &fdrv,
                band_drive: &band_drive,
                sweep_rate_hz_s: b.rate_hz_s,
                limiting: b.limiting,
            };
            let id = b.id.clone();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| law.iterate(&inputs))) {
                Ok(Ok(out)) => {
                    if out.drive.lines() == offs && out.drive.n() == m {
                        b.drive = Some(out.drive);
                    }
                }
                Ok(Err(e)) => {
                    trip.get_or_insert(RunEnd {
                        name: "law_fault",
                        aborted: true,
                        part: Some(id.clone()),
                        control: None,
                        drive: None,
                        words: format!("the band law of {id} failed: {}", e.0),
                    });
                }
                Err(_) => {
                    trip.get_or_insert(RunEnd {
                        name: "law_fault",
                        aborted: true,
                        part: Some(id.clone()),
                        control: None,
                        drive: None,
                        words: format!("the band law of {id} panicked"),
                    });
                }
            }
            // The band on its mean added level.
            let est = mean_diagonal(&estimate, setup.inner);
            let flo = mean_diagonal(&fref, setup.inner);
            let st = mean_diagonal(&inc, setup.inner);
            for i in 0..n {
                let added = est[i] - flo[i];
                let read = bdof >= MIN_ENVELOPE_DOF && st[i] > 0.0;
                b.error_db[i] = if read && added > 0.0 {
                    10.0 * (added / st[i]).log10()
                } else if read {
                    -99.0
                } else {
                    f64::NAN
                };
                let e = b.error_db[i];
                let (lo, hi) = b.abort_db[i];
                if b.ramp >= 1.0 && e.is_finite() && (e < -lo || e > hi) {
                    b.over_loops[i] += 1;
                } else {
                    b.over_loops[i] = 0;
                }
                if b.over_loops[i] >= BAND_ABORT_LOOPS {
                    trip.get_or_insert(RunEnd {
                        name: "band_abort",
                        aborted: true,
                        part: Some(id.clone()),
                        control: Some(i),
                        drive: None,
                        words: format!(
                            "band {id} read {e:.2} dB on control {i}, outside its abort band"
                        ),
                    });
                }
            }
        }
        if any {
            self.refactor = true;
        }
        if let Some(t) = trip {
            self.abort_with(t);
        }
    }
}
