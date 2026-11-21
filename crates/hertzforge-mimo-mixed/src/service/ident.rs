//! Identification (A4): MIMO Random's, played through the station's engine
//! by the run's own step with no tones. Flat uncorrelated random on every
//! drive, stepped from the start level until the controls sit
//! `below_reference_db` under the reference (no drive above its
//! identification limit), then recorded for `frames` Welch frames; the lag
//! is measured on the first step; `H` is H1 of the record, controls then
//! limits by drives.

use super::*;
use crate::tone::ToneBank;
use hertzforge_mimo_random::estimate::{best_lag, SdmEstimator};
use hertzforge_mimo_random::pool;
use hertzforge_sdm::linalg::Regularization;

/// What the step handed over while identifying: every response channel
/// (raw, controls then limits) and every drive played.
#[derive(Debug, Clone, Default)]
pub(crate) struct IdentRec {
    pub(crate) resp: Vec<Vec<f64>>,
    pub(crate) drive: Vec<Vec<f64>>,
}

impl IdentRec {
    fn new(q: usize, m: usize) -> Self {
        IdentRec {
            resp: vec![Vec::new(); q],
            drive: vec![Vec::new(); m],
        }
    }
}

impl MimoMixedService {
    /// The flat, uncorrelated identification drive: each drive `vrms` over
    /// the band (MIMO Random's).
    fn ident_drive(&self, vrms: &[f64]) -> HermitianStack {
        let m = self.cfg.m();
        let bw = (self.band.1 - self.band.0) as f64 * self.df;
        let mut g = HermitianStack::zeros(self.lines, m);
        for l in self.band.0..self.band.1 {
            for (k, v) in vrms.iter().enumerate().take(m) {
                g.set(l, k, k, C64::new(v * v / bw, 0.0));
            }
        }
        g
    }

    /// Play the identification drive in force until `frames` frames are
    /// recorded. `Err` when the run had to end (it has).
    fn ident_play(&mut self, frames: usize) -> Result<IdentRec, MimoMixedError> {
        let (q, m) = (self.cfg.n() + self.cfg.p(), self.cfg.m());
        self.ident_rec = Some(IdentRec::new(q, m));
        loop {
            self.top_up()?;
            if !self.run_batch()? {
                return Err(self.ended_error());
            }
            let got = self.take_records();
            self.t_s += got as f64 / self.fs;
            let trip = self.step.as_ref().and_then(|s| s.trip.clone());
            if let Some(end) = trip
                .map(|t| self.step_end(&t))
                .or_else(|| self.judge_drive_rms())
            {
                self.ident_rec = None;
                self.wind_down(end);
                return Err(self.ended_error());
            }
            if self
                .ident_rec
                .as_ref()
                .is_some_and(|r| r.resp.first().is_some_and(|x| x.len() >= frames))
            {
                return Ok(self.ident_rec.take().expect("record"));
            }
        }
    }

    /// Each limit channel of an identification estimate against its
    /// ceiling plus its abort margin at full level (MIMO Random's
    /// identification rule, the level at 0 dB: one line over aborts
    /// `limit_abort`). `Some` with the end when one is over.
    fn ident_limit_trip(&self, e: &hertzforge_mimo_random::estimate::Estimate) -> Option<RunEnd> {
        let (n, q) = (self.cfg.n(), self.cfg.n() + self.cfg.p());
        for (i, l) in self.cfg.limits.iter().enumerate() {
            let margin = 10f64.powf(l.abort_margin_db / 10.0);
            for line in self.band.0..self.band.1 {
                let c = l.ceiling(line);
                let v = e.auto[line * q + n + i];
                if c > 0.0 && v > c * margin {
                    let over_db = 10.0 * (v / c).log10();
                    return Some(RunEnd {
                        name: "limit_abort",
                        aborted: true,
                        part: None,
                        control: None,
                        drive: None,
                        words: format!(
                            "limit channel {} read {over_db:.1} dB over its ceiling at {:.1} Hz \
                             while identifying, past its {:.1} dB abort margin",
                            l.channel_index,
                            line as f64 * self.df,
                            l.abort_margin_db
                        ),
                    });
                }
            }
        }
        None
    }

    /// The error a run that ended reports.
    fn ended_error(&self) -> MimoMixedError {
        match self.end.as_ref() {
            Some(e) => MimoMixedError::Refused {
                name: e.name,
                part: e.part.clone().unwrap_or_default(),
                why: e.words.clone(),
            },
            None => MimoMixedError::BadState("the run ended".into()),
        }
    }

    /// Ramp the drive to zero, let the station play out, then end the run
    /// with `end` (the hardware freed first).
    fn wind_down(&mut self, end: RunEnd) {
        self.ending = None;
        self.begin_end(end, ABORT_RAMP_S);
        let mut left = DRAIN_BATCHES;
        for _ in 0..10_000 {
            let _ = self.top_up();
            match self.run_batch() {
                Ok(true) => {}
                _ => return,
            }
            let _ = self.take_records();
            if self.step.as_ref().is_some_and(|s| s.stopped()) {
                if left == 0 {
                    break;
                }
                left -= 1;
            }
        }
        if let Some(end) = self.ending.take() {
            self.finish(end);
        }
    }

    /// Ramp out and park the station (the identification's end).
    fn ident_park(&mut self) -> Result<(), MimoMixedError> {
        if let Some(s) = self.step.as_mut() {
            s.ramp_down(self.cfg.shutdown_ramp_s.max(ABORT_RAMP_S));
        }
        let mut left = DRAIN_BATCHES;
        for _ in 0..10_000 {
            self.top_up()?;
            if !self.run_batch()? {
                return Err(self.ended_error());
            }
            let _ = self.take_records();
            if self.step.as_ref().is_some_and(|s| s.stopped()) {
                if left == 0 {
                    break;
                }
                left -= 1;
            }
        }
        self.ident_rec = None;
        self.park();
        Ok(())
    }

    /// The controls' in-band level against the reference, dB.
    fn level_vs_reference(&self, g_cc: &HermitianStack) -> f64 {
        let n = self.cfg.n();
        let (mut a, mut r) = (0.0, 0.0);
        for l in self.band.0..self.band.1 {
            for i in 0..n {
                a += g_cc.get(l, i, i).re.max(0.0);
                r += self.cfg.reference.sdm.get(l, i, i).re.max(0.0);
            }
        }
        10.0 * (a / r).log10()
    }

    /// Estimate a record: drive frame `t` against response frame `t + lag`,
    /// from `skip`.
    fn ident_estimate(
        &self,
        rec: &IdentRec,
        skip: usize,
        lag: usize,
    ) -> hertzforge_mimo_random::estimate::Estimate {
        let n = self.cfg.n();
        let len = rec.resp[0].len().saturating_sub(skip + lag);
        let resp: Vec<&[f64]> = rec
            .resp
            .iter()
            .map(|r| &r[skip + lag..skip + lag + len])
            .collect();
        let drive: Vec<&[f64]> = rec.drive.iter().map(|d| &d[skip..skip + len]).collect();
        let est = SdmEstimator::with_window(
            self.nfft,
            self.fs,
            0.5,
            self.cfg.estimation.window,
            self.band,
        );
        est.estimate(&resp, n, &drive)
    }

    /// The identification: MIMO Random's level search and record, played
    /// through the station's engine; the station parked at its end.
    pub fn identify(&mut self) -> Result<&MimoMixedIdent, MimoMixedError> {
        if self.state != MimoMixedState::Armed {
            return Err(MimoMixedError::BadState(
                "identify once the station is armed".into(),
            ));
        }
        let (n, m, p) = (self.cfg.n(), self.cfg.m(), self.cfg.p());
        let q = n + p;
        let idc = self.cfg.identification();
        let bank = ToneBank::new(self.fs, &[], &[], m, q, self.cfg.tracking_hz, 0)
            .map_err(|e| MimoMixedError::BadState(e.to_string()))?;
        let mut step = self.build_step(bank, Vec::new())?;
        step.level = Ramp::at(1.0);
        step.rand_ramp = Ramp::at(0.0);
        step.rand_ramp.go(1.0, RANDOM_RAMP_S * self.fs);
        self.step = Some(step);
        let lc = self.loop_cfg.clone().expect("armed");
        self.synth = Some(DriveSynth::with_outputs(&lc, m));
        self.bands.clear();
        let mut vrms: Vec<f64> = self
            .cfg
            .drives
            .iter()
            .map(|d| idc.start_vrms.min(d.pretest_max_vrms))
            .collect();
        self.drive_full = Some(self.ident_drive(&vrms));
        self.refactor = true;
        self.top_up()?;
        self.unpark()?;
        self.fresh_engine()?;
        if let Err(e) = self.start_station("identification") {
            return Err(self.refuse_start(e));
        }
        let hop = self.nfft / 2;
        let settle = self.nfft + (RANDOM_RAMP_S * self.fs) as usize;
        let max_lag = self.nfft / 4;
        let search_frames = self.cfg.estimation.frames_per_loop.max(16);
        let nfft = self.nfft;
        let seg = |k: usize| nfft + hop * (k.max(1) - 1);
        let target = -idc.below_reference_db;
        let mut steps = 0usize;
        let mut lag = 0usize;
        let mut level;
        loop {
            let rec = self.ident_play(settle + seg(search_frames) + max_lag)?;
            if steps == 0 {
                // `best_lag` pairs each control window with the drive that
                // ends with it and starts `max_lag` frames earlier.
                let w = rec.drive[0].len() - settle - max_lag;
                let ctl: Vec<&[f64]> = rec.resp[..n.min(4)]
                    .iter()
                    .map(|r| &r[settle + max_lag..settle + max_lag + w])
                    .collect();
                lag = best_lag(&ctl, &rec.drive[0][settle..settle + w + max_lag], max_lag);
            }
            let e = self.ident_estimate(&rec, settle, lag);
            if let Some(end) = self.ident_limit_trip(&e) {
                self.wind_down(end);
                return Err(self.ended_error());
            }
            level = self.level_vs_reference(&e.g_cc);
            steps += 1;
            if (level - target).abs() <= idc.tolerance_db {
                break;
            }
            let at_limit = vrms
                .iter()
                .zip(&self.cfg.drives)
                .all(|(v, d)| *v >= d.pretest_max_vrms * (1.0 - 1e-9));
            if (level < target && at_limit) || steps >= idc.max_steps || !level.is_finite() {
                let words = format!(
                    "the controls reached {level:.1} dB against the reference after {steps} \
                     steps, short of {target:.1} dB within {:.1} dB, with each drive at most at \
                     its identification limit; raise the identification limits or check the \
                     wiring and sensitivities",
                    idc.tolerance_db
                );
                self.wind_down(RunEnd {
                    name: "ident_unreachable",
                    aborted: true,
                    part: None,
                    control: None,
                    drive: None,
                    words,
                });
                return Err(self.ended_error());
            }
            let gain = 10f64.powf(((target - level) / 20.0).clamp(-1.0, 1.0));
            for (v, d) in vrms.iter_mut().zip(&self.cfg.drives) {
                *v = (*v * gain).min(d.pretest_max_vrms);
            }
            self.drive_full = Some(self.ident_drive(&vrms));
            self.refactor = true;
            if let Some(s) = self.step.as_mut() {
                s.ring.clear();
            }
        }
        // The record.
        let rec = self.ident_play(self.nfft + seg(idc.frames) + max_lag)?;
        let e = self.ident_estimate(&rec, self.nfft, lag);
        if let Some(end) = self.ident_limit_trip(&e) {
            self.wind_down(end);
            return Err(self.ended_error());
        }
        level = self.level_vs_reference(&e.g_cc);
        drop(rec);
        self.ident_park()?;
        let band = self.band;
        fn la<T>(r: Result<T, hertzforge_sdm::linalg::LinalgError>) -> Result<T, MimoMixedError> {
            r.map_err(|e| MimoMixedError::Law(format!("the identification: {e}")))
        }
        let h = la(pool::h1_frf(&e.g_rd, &e.g_dd, Regularization::None, band))?;
        let coherence = la(pool::multiple_coherence(
            &e.g_cc,
            &rows(&e.g_rd, 0..n),
            &e.g_dd,
            Regularization::None,
            band,
        ))?;
        let limit_coherence = if p > 0 {
            let mut gll = HermitianStack::zeros(self.lines, p);
            for l in 0..self.lines {
                for i in 0..p {
                    gll.set(l, i, i, C64::new(e.auto[l * q + n + i], 0.0));
                }
            }
            la(pool::multiple_coherence(
                &gll,
                &rows(&e.g_rd, n..q),
                &e.g_dd,
                Regularization::None,
                band,
            ))?
        } else {
            Vec::new()
        };
        let band_coherence = (0..n)
            .map(|i| {
                let s: f64 = (band.0..band.1).map(|l| coherence[l * n + i]).sum();
                s / (band.1 - band.0).max(1) as f64
            })
            .collect();
        let frames_ahead = self
            .starts
            .iter()
            .rev()
            .find(|s| s.phase == "identification")
            .map_or(0, |s| s.frames_ahead());
        self.ident = Some(MimoMixedIdent {
            h,
            coherence,
            limit_coherence,
            g_cc: e.g_cc.clone(),
            g_dd: e.g_dd.clone(),
            frames: e.frames,
            level_db: level,
            drive_vrms: vrms,
            steps,
            lag_frames: lag,
            frames_ahead,
            band_coherence,
        });
        self.step = None;
        self.synth = None;
        self.factor = None;
        self.drive_full = None;
        self.state = MimoMixedState::Identified;
        self.note(format!(
            "Identified at {level:.1} dB against the reference after {steps} steps; lag \
             {lag} frames; {} frames ahead of the first drive frame.",
            frames_ahead
        ));
        Ok(self.ident.as_ref().expect("ident"))
    }
}
