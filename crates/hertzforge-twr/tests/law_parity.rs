//! The seam against the run without it: every fixture of the recorded trace
//! run twice on identical stations, without the seam (`control: None`) and
//! under it with the built-in law (`twr_correct` from the same correction
//! settings, the at-rest taper off, every other guard at its default).
//!
//! The seam works at full level and today's iteration at the level, the same
//! arithmetic reordered, so the two agree to rounding: the 0 dB fixture (b),
//! where the level's scale is exactly 1, bit for bit; every other drive sample
//! within 1e-12 of its record's peak while the station has measured the same
//! responses, bit for bit, on every play before it; every discrete outcome
//! exact (each
//! step's state, level and rung, each play's kind, level, rung, iteration,
//! gate and clipped samples, the station frames, the end state, the trip and
//! the abort sentence). The metrics are held to a relative 1e-6 (the spectral
//! error is a mean of log ratios, which magnifies a rounding difference on a
//! bin near a null); the worst is printed. Clamp counts are compared and a flip
//! on a borderline bin is reported, not gated.
//!
//! **Why the responses decide the tolerance.** The station plays `f32`
//! samples. Two drives a rounding apart in `f64` almost always convert to the
//! same `f32` record, and then the station measures the same response and the
//! runs stay a rounding apart. Once one sample lands on either side of an
//! `f32` rounding boundary, the responses differ by the `f32` resolution
//! (about 6e-8 of a sample), and every later drive inherits a difference of
//! that size. After that first split the drives are held to 1e-6 of their peak
//! and the discrete outcomes must still be exact; the split play is printed.
//! On the clip fixture (d) the first split is at play 8 (its drives until then
//! within 7e-14 of peak, its responses bit for bit); after it the worst is
//! about 2e-8.

mod common;

use common::{fixtures, player, Fixture, Step};
use hertzforge_hal::Backend;
use hertzforge_twr::{BackendReferencePlayer, PlayRow, TwrControl, TwrService, TwrState};

/// What one run did: each step's (state, level, rung), each play's row and
/// drive record, and the end.
#[derive(Debug)]
struct Run {
    steps: Vec<(TwrState, f64, Option<usize>)>,
    plays: Vec<(PlayRow, Vec<f64>)>,
    /// Each play's measured response.
    resp: Vec<Vec<f64>>,
    end: (TwrState, u64, Option<String>, Option<String>),
}

fn terminal(s: TwrState) -> bool {
    matches!(
        s,
        TwrState::Complete | TwrState::Aborted | TwrState::Stopped | TwrState::Holding
    )
}

fn run(f: &Fixture, seam: bool) -> Run {
    let mut cfg = f.cfg.clone();
    if seam {
        cfg.control = Some(TwrControl {
            rest_taper: false,
            ..TwrControl::default()
        });
    }
    let mut svc = TwrService::new(player(f.station.backend()), cfg).unwrap();
    let mut out = Run {
        steps: Vec::new(),
        plays: Vec::new(),
        resp: Vec::new(),
        end: (TwrState::Configured, 0, None, None),
    };
    let advance = |svc: &mut TwrService<BackendReferencePlayer<Box<dyn Backend>>>,
                   out: &mut Run| {
        let r = svc.advance();
        if svc.state() == TwrState::Review {
            svc.proceed().unwrap();
        }
        let have = out.plays.len();
        for row in &svc.plays()[have..] {
            out.plays.push((row.clone(), svc.drive_time().to_vec()));
            out.resp.push(svc.achieved_time().to_vec());
        }
        r
    };
    for step in &f.script {
        match *step {
            Step::Advance => {
                let _ = advance(&mut svc, &mut out);
            }
            Step::UntilGated(max) => {
                for _ in 0..max {
                    advance(&mut svc, &mut out).unwrap();
                    if svc.status().converged_at_level || terminal(svc.state()) {
                        break;
                    }
                }
            }
            Step::ToEnd(max) => {
                for _ in 0..max {
                    if terminal(svc.state()) {
                        break;
                    }
                    advance(&mut svc, &mut out).unwrap();
                }
            }
            Step::Up => {
                let _ = svc.step_level_up();
            }
            Step::Jump(i) => {
                let _ = svc.jump_to_level(i);
            }
            Step::Play => {
                let _ = svc.begin_playback();
            }
        }
        let st = svc.status();
        out.steps.push((st.state, st.level_db, st.rung));
    }
    let st = svc.status();
    out.end = (
        st.state,
        st.station_frames,
        st.trip
            .as_ref()
            .map(|t| format!("{} {:?}", t.kind.as_str(), t.play)),
        st.abort_reason.clone(),
    );
    out
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-300)
}

#[test]
fn the_seam_with_the_built_in_is_the_run_without_it() {
    let mut report = Vec::new();
    for f in fixtures() {
        let (open, seam) = (run(&f, false), run(&f, true));
        let tag = f.tag;
        assert_eq!(open.steps, seam.steps, "{tag}: the steps");
        assert_eq!(open.end, seam.end, "{tag}: the end");
        assert_eq!(open.plays.len(), seam.plays.len(), "{tag}: the plays");
        let (mut worst, mut worst_metric, mut flips) = (0.0f64, 0.0f64, 0usize);
        let mut split: Option<u64> = None;
        let mut worst_after = 0.0f64;
        for (((a, da), (b, db)), (ya, yb)) in open
            .plays
            .iter()
            .zip(&seam.plays)
            .zip(open.resp.iter().zip(&seam.resp))
        {
            let i = a.index;
            assert_eq!(
                (
                    a.kind,
                    a.level_db,
                    a.rung,
                    a.iteration,
                    a.gated,
                    a.clipped_samples
                ),
                (
                    b.kind,
                    b.level_db,
                    b.rung,
                    b.iteration,
                    b.gated,
                    b.clipped_samples
                ),
                "{tag}: play {i}"
            );
            assert_eq!(
                (a.start_frame, a.frames),
                (b.start_frame, b.frames),
                "{tag}: play {i}"
            );
            for (x, y, what) in [
                (a.metrics.rms_error_pct, b.metrics.rms_error_pct, "rms"),
                (a.metrics.peak_error_pct, b.metrics.peak_error_pct, "peak"),
                (
                    a.metrics.spectral_error_db,
                    b.metrics.spectral_error_db,
                    "spectral",
                ),
                (
                    a.metrics.response_rms,
                    b.metrics.response_rms,
                    "response rms",
                ),
                (a.metrics.drive_rms, b.metrics.drive_rms, "drive rms"),
            ] {
                assert!(close(x, y, 1e-6), "{tag}: play {i}: {what} {x} against {y}");
                worst_metric = worst_metric.max((x - y).abs() / x.abs().max(y.abs()).max(1e-300));
            }
            if a.clamped_bins != b.clamped_bins {
                flips += 1;
            }
            let pk = da.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            assert_eq!(da.len(), db.len());
            for (k, (x, y)) in da.iter().zip(db).enumerate() {
                if tag == "b" {
                    assert_eq!(
                        x.to_bits(),
                        y.to_bits(),
                        "{tag}: play {i} sample {k}: {x} {y}"
                    );
                }
                let d = (x - y).abs() / pk.max(f64::MIN_POSITIVE);
                let tol = if split.is_some() { 1e-6 } else { 1e-12 };
                assert!(
                    d <= tol,
                    "{tag}: play {i} sample {k}: {x} {y} ({d:e} of peak, responses split at \
                     {split:?})"
                );
                if split.is_some() {
                    worst_after = worst_after.max(d);
                } else {
                    worst = worst.max(d);
                }
            }
            let same =
                ya.len() == yb.len() && ya.iter().zip(yb).all(|(p, q)| p.to_bits() == q.to_bits());
            if !same && split.is_none() {
                assert_ne!(tag, "b", "the 0 dB fixture plays the same records");
                split = Some(i);
            }
        }
        report.push(format!(
            "{tag}: {} plays, end {:?}, worst drive difference {worst:.2e} of peak, responses \
             split at {split:?} (worst after {worst_after:.2e}), worst metric \
             {worst_metric:.2e} relative, {flips} clamp-count flips",
            open.plays.len(),
            open.end.0
        ));
    }
    for line in &report {
        println!("{line}");
    }
}
