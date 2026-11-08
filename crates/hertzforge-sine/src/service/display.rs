//! The run's display record: what a screen draws, kept by the control tick.
//!
//! Two views of the same per-tick numbers:
//!
//! - **Against frequency** (a sweep or a step test): a log-spaced grid of
//!   [`DISPLAY_GRID_POINTS`] across the run's adopted band, each point holding
//!   the LATEST tick that was at it. A sweep up then down draws the down pass
//!   over the up pass, and a new level draws over the last one, which is what a
//!   screen following the run should show. A tick that moved the tone across
//!   several points (a fast sweep at the low end) fills the points between by
//!   linear interpolation from the tick before; a move the operator commanded
//!   (a jog or a go to) does not, because the tone did not pass through them.
//! - **Against time** (a dwell): the last [`DISPLAY_HISTORY_S`] seconds of
//!   station time, at most [`DISPLAY_HISTORY_POINTS`] points (every tick at a
//!   tick of 50 ms or longer; every n-th tick below that).
//!
//! Both are kept for every test kind; the screen picks. The grid is
//! log-spaced because every Sine plot has a log frequency axis: a linear grid
//! of 256 points over 20 to 2000 Hz puts three points under 40 Hz.
//!
//! # Cost on the control tick
//!
//! One row of `11 + 3 * inputs + limits` numbers per tick, written into
//! buffers sized when the step is built: nothing is allocated per tick. The
//! write takes the lock with `try_lock`, so a reader copying the record out
//! can never hold the control tick up; a tick that finds it busy is skipped,
//! and the next one fills the points between.

use crate::service::report::FrfDisplay;
use std::sync::Mutex;

/// Points on the frequency grid.
pub const DISPLAY_GRID_POINTS: usize = 256;
/// Seconds of station time the time history covers.
pub const DISPLAY_HISTORY_S: f64 = 60.0;
/// Most points the time history holds.
pub const DISPLAY_HISTORY_POINTS: usize = 1200;

const F_FREQ: usize = 0;
const F_LEVEL: usize = 1;
const F_MEASURED: usize = 2;
const F_TARGET: usize = 3;
const F_ALARM_LO: usize = 4;
const F_ALARM_HI: usize = 5;
const F_ABORT_LO: usize = 6;
const F_ABORT_HI: usize = 7;
const F_DRIVE: usize = 8;
const F_COLA: usize = 9;
const F_THD: usize = 10;
const N_FIXED: usize = 11;

/// One tick's numbers, borrowed from the step's own buffers.
pub(crate) struct DisplayRow<'a> {
    /// Station time of the tick, seconds since engage, holds included.
    pub t_s: f64,
    /// The frequency the tick's amplitudes were measured at, Hz.
    pub freq_hz: f64,
    /// The scheduled level, dB.
    pub level_db: f64,
    /// The control composite, control unit peak.
    pub measured: f64,
    /// The profile at this frequency and level.
    pub target: f64,
    /// The alarm band at this frequency and level (NaN outside the profile).
    pub alarm: (f64, f64),
    /// The abort band at this frequency and level (NaN outside the profile).
    pub abort: (f64, f64),
    /// The drive as played, drive units peak.
    pub drive_vpk: f64,
    /// The reference output's amplitude, volts peak; NaN without one.
    pub cola_vpk: f64,
    /// THD on the control channel, a fraction.
    pub thd: f64,
    /// Per input slot.
    pub tracked: &'a [f64],
    pub phase_deg: &'a [f64],
    pub rms: &'a [f64],
    /// Per limit channel: the bound at this frequency.
    pub limit_bound: &'a [f64],
}

/// The record itself. Lives behind a mutex shared by the step and the service.
pub(crate) struct DisplayLog {
    n_in: usize,
    n_lim: usize,
    width: usize,
    lo_ln: f64,
    span_ln: f64,
    grid_hz: Vec<f64>,
    grid: Vec<f32>,
    visited: Vec<bool>,
    last_bin: Option<usize>,
    last_row: Vec<f32>,
    row: Vec<f32>,
    jumped: bool,
    stride: u32,
    since: u32,
    hist_t: Vec<f64>,
    hist: Vec<f32>,
    head: usize,
    len: usize,
}

impl DisplayLog {
    /// A record for `n_in` input slots and `n_lim` limit channels over `band`
    /// at a control tick of `tick_s`.
    pub(crate) fn new(n_in: usize, n_lim: usize, band: (f64, f64), tick_s: f64) -> Self {
        let (lo, hi) = display_band(band);
        let width = N_FIXED + 3 * n_in + n_lim;
        let lo_ln = lo.ln();
        let span_ln = (hi.ln() - lo_ln).max(1e-12);
        let n = DISPLAY_GRID_POINTS;
        // The ends are the band's own numbers, not `exp(ln(x))` of them: a grid
        // point a rounding past the band's edge would fall outside the profile.
        let grid_hz = (0..n)
            .map(|k| match k {
                0 => lo,
                k if k == n - 1 => hi,
                k => (lo_ln + span_ln * k as f64 / (n - 1) as f64).exp(),
            })
            .collect();
        let per_point = DISPLAY_HISTORY_S / DISPLAY_HISTORY_POINTS as f64;
        let stride = if tick_s.is_finite() && tick_s > 0.0 {
            (per_point / tick_s).ceil().max(1.0) as u32
        } else {
            1
        };
        DisplayLog {
            n_in,
            n_lim,
            width,
            lo_ln,
            span_ln,
            grid_hz,
            grid: vec![f32::NAN; n * width],
            visited: vec![false; n],
            last_bin: None,
            last_row: vec![f32::NAN; width],
            row: vec![f32::NAN; width],
            jumped: false,
            stride,
            since: 0,
            hist_t: vec![0.0; DISPLAY_HISTORY_POINTS],
            hist: vec![f32::NAN; DISPLAY_HISTORY_POINTS * width],
            head: 0,
            len: 0,
        }
    }

    /// The next tick's frequency move was the operator's (a jog or a go to):
    /// do not fill the points between.
    pub(crate) fn mark_jump(&mut self) {
        self.jumped = true;
    }

    fn bin(&self, f: f64) -> usize {
        let n = DISPLAY_GRID_POINTS;
        let x = (f.ln() - self.lo_ln) / self.span_ln * (n - 1) as f64;
        if x.is_nan() {
            return 0;
        }
        (x.round().max(0.0) as usize).min(n - 1)
    }

    /// Record one tick.
    pub(crate) fn write(&mut self, r: &DisplayRow<'_>) {
        let w = self.width;
        {
            let row = &mut self.row;
            row[F_FREQ] = r.freq_hz as f32;
            row[F_LEVEL] = r.level_db as f32;
            row[F_MEASURED] = r.measured as f32;
            row[F_TARGET] = r.target as f32;
            row[F_ALARM_LO] = r.alarm.0 as f32;
            row[F_ALARM_HI] = r.alarm.1 as f32;
            row[F_ABORT_LO] = r.abort.0 as f32;
            row[F_ABORT_HI] = r.abort.1 as f32;
            row[F_DRIVE] = r.drive_vpk as f32;
            row[F_COLA] = r.cola_vpk as f32;
            row[F_THD] = r.thd as f32;
            for s in 0..self.n_in {
                let base = N_FIXED + 3 * s;
                row[base] = r.tracked.get(s).copied().unwrap_or(f64::NAN) as f32;
                row[base + 1] = r.phase_deg.get(s).copied().unwrap_or(f64::NAN) as f32;
                row[base + 2] = r.rms.get(s).copied().unwrap_or(f64::NAN) as f32;
            }
            for l in 0..self.n_lim {
                row[N_FIXED + 3 * self.n_in + l] =
                    r.limit_bound.get(l).copied().unwrap_or(f64::NAN) as f32;
            }
        }

        // Against frequency: the latest tick at each point.
        if r.freq_hz.is_finite() && r.freq_hz > 0.0 {
            let k = self.bin(r.freq_hz);
            if let (Some(last), false) = (self.last_bin, self.jumped) {
                let gap = k.abs_diff(last);
                if gap > 1 {
                    let (from, to) = (last as f64, k as f64);
                    let lo = last.min(k) + 1;
                    let hi = last.max(k);
                    for j in lo..hi {
                        let t = ((j as f64 - from) / (to - from)) as f32;
                        let dst = &mut self.grid[j * w..(j + 1) * w];
                        for (c, d) in dst.iter_mut().enumerate() {
                            let a = self.last_row[c];
                            *d = a + t * (self.row[c] - a);
                        }
                        self.visited[j] = true;
                    }
                }
            }
            self.grid[k * w..(k + 1) * w].copy_from_slice(&self.row);
            self.visited[k] = true;
            self.last_bin = Some(k);
            self.last_row.copy_from_slice(&self.row);
            self.jumped = false;
        }

        // Against time: every `stride`-th tick.
        if self.since == 0 {
            let cap = DISPLAY_HISTORY_POINTS;
            let at = (self.head + self.len) % cap;
            self.hist_t[at] = r.t_s;
            self.hist[at * w..(at + 1) * w].copy_from_slice(&self.row);
            if self.len < cap {
                self.len += 1;
            } else {
                self.head = (self.head + 1) % cap;
            }
        }
        self.since = (self.since + 1) % self.stride.max(1);
    }

    fn curves_from(&self, rows: &[&[f32]], slots: &DisplaySlots) -> SineCurves {
        let col = |c: usize| -> Vec<f32> { rows.iter().map(|r| r[c]).collect() };
        let cola = col(F_COLA);
        let inputs = (0..self.n_in)
            .map(|s| {
                let base = N_FIXED + 3 * s;
                let limit = slots
                    .limit_slots
                    .iter()
                    .position(|&ls| ls == s)
                    .map(|l| col(N_FIXED + 3 * self.n_in + l));
                SineInputCurves {
                    channel_index: slots.input_index.get(s).copied().unwrap_or(s as u32),
                    is_control: slots.control_slots.contains(&s),
                    tracked: col(base),
                    phase_deg: col(base + 1),
                    rms: col(base + 2),
                    limit,
                }
            })
            .collect();
        SineCurves {
            freq_hz: col(F_FREQ),
            level_db: col(F_LEVEL),
            measured: col(F_MEASURED),
            target: col(F_TARGET),
            alarm_lo: col(F_ALARM_LO),
            alarm_hi: col(F_ALARM_HI),
            abort_lo: col(F_ABORT_LO),
            abort_hi: col(F_ABORT_HI),
            drive_vpk: col(F_DRIVE),
            cola_vpk: slots.cola.then_some(cola),
            thd: col(F_THD),
            inputs,
        }
    }

    /// Copy the record out (allocates; between batches only).
    pub(crate) fn snapshot(
        &self,
        slots: &DisplaySlots,
    ) -> (Vec<f64>, Vec<bool>, SineCurves, Vec<f64>, SineCurves) {
        let w = self.width;
        let grid_rows: Vec<&[f32]> = self.grid.chunks(w).collect();
        let sweep = self.curves_from(&grid_rows, slots);
        let cap = DISPLAY_HISTORY_POINTS;
        let mut t = Vec::with_capacity(self.len);
        let mut hist_rows: Vec<&[f32]> = Vec::with_capacity(self.len);
        for i in 0..self.len {
            let at = (self.head + i) % cap;
            t.push(self.hist_t[at]);
            hist_rows.push(&self.hist[at * w..(at + 1) * w]);
        }
        let history = self.curves_from(&hist_rows, slots);
        (
            self.grid_hz.clone(),
            self.visited.clone(),
            sweep,
            t,
            history,
        )
    }
}

/// The band the grid spans: the run's, widened a little when it is one
/// frequency (a dwell), so the grid is never empty.
fn display_band(band: (f64, f64)) -> (f64, f64) {
    let (lo, hi) = band;
    if lo.is_finite() && hi.is_finite() && lo > 0.0 && hi > lo * (1.0 + 1e-9) {
        (lo, hi)
    } else if lo.is_finite() && lo > 0.0 {
        (lo * 0.999, lo * 1.001)
    } else {
        (1.0, 2.0)
    }
}

/// Which input slot is which channel, kept beside the record.
#[derive(Debug, Clone, Default)]
pub(crate) struct DisplaySlots {
    pub input_index: Vec<u32>,
    pub control_slots: Vec<usize>,
    pub limit_slots: Vec<usize>,
    pub cola: bool,
}

/// The shared cell the step writes and the service reads
/// ([`crate::SineService::display`]).
pub struct DisplayCell {
    pub(crate) log: Mutex<DisplayLog>,
    pub(crate) slots: Mutex<DisplaySlots>,
}

impl DisplayCell {
    /// Name each input slot by its channel-table index (the step numbers
    /// them 0, 1, ... until the service, which holds the table, says).
    pub fn set_input_indices(&self, indices: &[u32]) {
        if let Ok(mut s) = self.slots.lock() {
            for (slot, &ix) in s.input_index.iter_mut().zip(indices) {
                *slot = ix;
            }
        }
    }
}

/// One input's curves.
#[derive(Debug, Clone, PartialEq)]
pub struct SineInputCurves {
    /// The channel-table index.
    pub channel_index: u32,
    /// Whether the channel is one of the control rows.
    pub is_control: bool,
    /// Tracked amplitude at the tone's frequency, the channel's unit, peak.
    pub tracked: Vec<f32>,
    /// Phase against the drive, degrees.
    pub phase_deg: Vec<f32>,
    /// Broadband rms over the tick, the channel's unit.
    pub rms: Vec<f32>,
    /// For a limit channel, its bound (the limit profile's abort edge at the
    /// frequency), the channel's unit, peak.
    pub limit: Option<Vec<f32>>,
}

/// One set of curves: on the frequency grid or on the time history. Every
/// vector has one value per point. On the grid a point the run has not been
/// at is NaN in every vector.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SineCurves {
    /// The frequency each point was measured at, Hz.
    pub freq_hz: Vec<f32>,
    /// The scheduled level, dB.
    pub level_db: Vec<f32>,
    /// The control composite, control unit peak.
    pub measured: Vec<f32>,
    /// The profile at the point's frequency and level.
    pub target: Vec<f32>,
    /// The alarm and abort bands at the point's frequency and level.
    pub alarm_lo: Vec<f32>,
    pub alarm_hi: Vec<f32>,
    pub abort_lo: Vec<f32>,
    pub abort_hi: Vec<f32>,
    /// The drive as played, drive units peak.
    pub drive_vpk: Vec<f32>,
    /// The reference output's amplitude, volts peak; `None` without one.
    pub cola_vpk: Option<Vec<f32>>,
    /// THD on the control channel, a fraction.
    pub thd: Vec<f32>,
    /// Every enabled input, in table order.
    pub inputs: Vec<SineInputCurves>,
}

/// Everything a Sine screen draws, against frequency and against time.
///
/// Built between batches by [`crate::SineService::display`]. Empty curves
/// (no grid points visited, no history) until the loop has engaged.
#[derive(Debug, Clone, PartialEq)]
pub struct SineDisplay {
    /// The frequency grid, Hz: [`DISPLAY_GRID_POINTS`] log-spaced points
    /// across the adopted band.
    pub grid_hz: Vec<f64>,
    /// Whether the run has been at each grid point.
    pub visited: Vec<bool>,
    /// The profile at the present level on the grid, and its alarm and abort
    /// bands, computed from the profile now (not when each point was visited).
    pub reference: Vec<f32>,
    pub alarm_lo: Vec<f32>,
    pub alarm_hi: Vec<f32>,
    pub abort_lo: Vec<f32>,
    pub abort_hi: Vec<f32>,
    /// The latest tick at each grid point.
    pub sweep: SineCurves,
    /// Station time of each history point, seconds since engage, oldest
    /// first; the last [`DISPLAY_HISTORY_S`] seconds.
    pub history_t_s: Vec<f64>,
    /// The time history.
    pub history: SineCurves,
    /// The identification on its own grid (`|H|`, coherence, the trust mask);
    /// `None` until identification has run.
    pub identification: Option<FrfDisplay>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(log: &mut DisplayLog, t: f64, f: f64, v: f64) {
        log.write(&DisplayRow {
            t_s: t,
            freq_hz: f,
            level_db: 0.0,
            measured: v,
            target: 1.0,
            alarm: (0.8, 1.2),
            abort: (0.7, 1.4),
            drive_vpk: 0.1,
            cola_vpk: f64::NAN,
            thd: 0.01,
            tracked: &[v],
            phase_deg: &[-10.0],
            rms: &[v * 0.7],
            limit_bound: &[],
        });
    }

    fn slots() -> DisplaySlots {
        DisplaySlots {
            input_index: vec![1],
            control_slots: vec![0],
            limit_slots: vec![],
            cola: false,
        }
    }

    #[test]
    fn the_grid_is_log_spaced_across_the_band() {
        let log = DisplayLog::new(1, 0, (20.0, 2000.0), 0.01);
        let (g, ..) = log.snapshot(&slots());
        assert_eq!(g.len(), DISPLAY_GRID_POINTS);
        assert!((g[0] - 20.0).abs() < 1e-9 && (g[255] - 2000.0).abs() < 1e-6);
        // Constant ratio: a log grid. A linear one of 256 points would put
        // three under 40 Hz; this one puts 38 there.
        let r = g[1] / g[0];
        assert!(g.windows(2).all(|p| (p[1] / p[0] - r).abs() < 1e-9));
        assert!(g.iter().filter(|&&f| f < 40.0).count() >= 30);
    }

    #[test]
    fn each_point_holds_the_latest_tick_and_a_sweep_fills_the_points_it_crossed() {
        let mut log = DisplayLog::new(1, 0, (100.0, 800.0), 0.01);
        write(&mut log, 0.0, 100.0, 1.0);
        // A move of about ten points in one tick, as a fast sweep makes.
        let f2 = 100.0 * (8f64.ln() * 10.0 / 255.0).exp();
        write(&mut log, 0.01, f2, 2.0);
        let (_, visited, sweep, ..) = log.snapshot(&slots());
        assert!(
            visited[..=10].iter().all(|&v| v),
            "the crossed points are filled"
        );
        assert!(!visited[11], "the run has not been past the last tick");
        assert!((sweep.measured[0] - 1.0).abs() < 1e-6);
        assert!((sweep.measured[10] - 2.0).abs() < 1e-6);
        assert!(
            (sweep.measured[5] - 1.5).abs() < 1e-3,
            "{}",
            sweep.measured[5]
        );
        assert!(sweep.measured[11].is_nan());
        // A later pass over the first point replaces it.
        write(&mut log, 0.02, 100.0, 3.0);
        let (_, _, sweep, ..) = log.snapshot(&slots());
        assert!((sweep.measured[0] - 3.0).abs() < 1e-6);
        assert_eq!(sweep.inputs[0].channel_index, 1);
        assert!(sweep.inputs[0].is_control);
        assert!(sweep.cola_vpk.is_none());
    }

    #[test]
    fn an_operator_move_does_not_fill_the_points_between() {
        let mut log = DisplayLog::new(1, 0, (100.0, 800.0), 0.01);
        write(&mut log, 0.0, 100.0, 1.0);
        log.mark_jump();
        write(&mut log, 0.01, 400.0, 2.0);
        let (_, visited, ..) = log.snapshot(&slots());
        assert_eq!(visited.iter().filter(|&&v| v).count(), 2);
    }

    #[test]
    fn the_history_keeps_the_last_sixty_seconds() {
        // A 10 ms tick: a point every fifth tick, 1200 points over 60 s.
        let mut log = DisplayLog::new(1, 0, (250.0, 250.0), 0.01);
        for i in 0..20_000 {
            write(&mut log, i as f64 * 0.01, 250.0, i as f64);
        }
        let (.., t, hist) = log.snapshot(&slots());
        assert_eq!(t.len(), DISPLAY_HISTORY_POINTS);
        let span = t[t.len() - 1] - t[0];
        assert!((span - 59.95).abs() < 1e-6, "{span}");
        assert!(t.windows(2).all(|p| p[1] > p[0]), "oldest first");
        assert_eq!(hist.measured.len(), t.len());
    }
}
