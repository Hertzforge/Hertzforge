//! The core's checks on a control law's proposed output: the pure half of "the
//! law proposes, the core disposes". The service owns the orchestration (what is
//! held, what is counted, when the run stops); this module holds the arithmetic,
//! so each rule is testable on its own.

use crate::control::LoopOutputs;
use crate::service::report::LawFaultReason;

/// Check a law's proposed output before the core uses any of it.
///
/// Rejects a drive of the wrong length, any non-finite value, any negative
/// value, and a non-finite level command (in that order; the first failing rule
/// names the reason). An accepted drive has every line outside the control band
/// forced to zero: the loop never drives where it does not control.
pub(crate) fn check_output(
    mut out: LoopOutputs,
    nbins: usize,
    band: (usize, usize),
) -> Result<LoopOutputs, LawFaultReason> {
    if out.drive_psd.len() != nbins {
        return Err(LawFaultReason::WrongLength);
    }
    if out.drive_psd.iter().any(|p| !p.is_finite()) {
        return Err(LawFaultReason::NonFinite);
    }
    if out.drive_psd.iter().any(|&p| p < 0.0) {
        return Err(LawFaultReason::Negative);
    }
    if !out.level_cmd_db.is_finite() {
        return Err(LawFaultReason::NonFiniteLevel);
    }
    let (lo, hi) = band;
    for (k, p) in out.drive_psd.iter_mut().enumerate() {
        if k < lo || k >= hi {
            *p = 0.0;
        }
    }
    Ok(out)
}

/// How far below the in-band mean of the drive in force a line counts as
/// "nothing", dB. The change cap measures a rise from at least this floor, so a
/// line the law had at (or near) zero can still be brought up, one cap step a
/// loop, instead of being pinned at zero forever.
pub(crate) const CAP_FLOOR_DB: f64 = -60.0;

/// What the per-loop change cap did to one proposed drive.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CapReport {
    /// In-band lines the cap held back on this call.
    pub bound: Vec<usize>,
    /// The largest change any in-band line asked for, dB, before the cap: a rise
    /// from the reference (or the floor, for a line under it), a fall from the
    /// reference, over lines whose proposal is above zero. A line the cap binds
    /// asked for more than the cap.
    pub peak_db: f64,
}

/// Cap how far each in-band line of `proposed` may move from `reference` (the
/// drive in force, already carried to any commanded level), in place.
///
/// A line may rise to at most `max(reference, floor) * 10^(cap/10)` and fall to
/// at least `reference / 10^(cap/10)`, where the floor is [`CAP_FLOOR_DB`] below
/// the reference's in-band mean. Lines outside the band are left alone (the
/// output check has already zeroed them).
pub(crate) fn cap_change(
    proposed: &mut [f64],
    reference: &[f64],
    band: (usize, usize),
    cap_db: f64,
) -> CapReport {
    let n = proposed.len().min(reference.len());
    let (lo, hi) = (band.0.min(n), band.1.min(n));
    let mut report = CapReport::default();
    if lo >= hi {
        return report;
    }
    let ratio = 10f64.powf(cap_db / 10.0);
    let mean = reference[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
    let floor = mean * 10f64.powf(CAP_FLOOR_DB / 10.0);
    for k in lo..hi {
        let r = reference[k];
        let base = r.max(floor);
        let p = proposed[k];
        // The change asked for, measured the way the cap judges it: a rise from
        // the base (the reference, or the floor under it), a fall from the
        // reference. A line the cap binds asked for more than `cap_db`.
        let asked_db = if p > base && base > 0.0 {
            10.0 * (p / base).log10()
        } else if p < r && p > 0.0 {
            10.0 * (r / p).log10()
        } else {
            0.0
        };
        report.peak_db = report.peak_db.max(asked_db);
        let up = base * ratio;
        let down = r / ratio;
        if p > up {
            proposed[k] = up;
            report.bound.push(k);
        } else if p < down {
            proposed[k] = down;
            report.bound.push(k);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::LoopDiagnostics;

    #[test]
    fn the_cap_holds_each_line_within_its_step_and_names_the_lines() {
        // 6 dB cap (x3.98): band [1, 5).
        let reference = vec![9.0, 1.0, 1.0, 1.0, 1.0, 9.0];
        let mut p = vec![100.0, 3.0, 10.0, 0.1, 0.0, 100.0];
        let r = cap_change(&mut p, &reference, (1, 5), 6.0);
        let x = 10f64.powf(0.6);
        assert_eq!(p[0], 100.0, "out of band untouched");
        assert_eq!(p[5], 100.0, "out of band untouched");
        assert_eq!(p[1], 3.0, "inside the cap");
        assert!((p[2] - x).abs() < 1e-12, "held to +6 dB");
        assert!((p[3] - 1.0 / x).abs() < 1e-12, "held to -6 dB");
        assert!(
            (p[4] - 1.0 / x).abs() < 1e-12,
            "a line cut to zero is held to -6 dB"
        );
        assert_eq!(r.bound, vec![2, 3, 4]);
        assert!(
            (r.peak_db - 10.0).abs() < 1e-9,
            "10 dB asked for: {}",
            r.peak_db
        );
    }

    #[test]
    fn a_line_at_zero_can_rise_from_the_floor_one_step_at_a_time() {
        let reference = vec![1.0, 0.0, 1.0];
        let mut p = vec![1.0, 1.0, 1.0];
        let r = cap_change(&mut p, &reference, (0, 3), 6.0);
        let floor = (2.0 / 3.0) * 1e-6;
        assert!((p[1] - floor * 10f64.powf(0.6)).abs() < 1e-18);
        assert_eq!(r.bound, vec![1]);
    }

    #[test]
    fn a_proposal_equal_to_the_reference_is_not_capped() {
        let reference = vec![0.5, 2.0, 1e-9];
        let mut p = reference.clone();
        let r = cap_change(&mut p, &reference, (0, 3), 6.0);
        assert_eq!(p, reference);
        assert!(r.bound.is_empty());
        assert_eq!(r.peak_db, 0.0);
    }

    fn out(drive: Vec<f64>, level: f64) -> LoopOutputs {
        LoopOutputs {
            drive_psd: drive,
            level_cmd_db: level,
            diagnostics: LoopDiagnostics::default(),
        }
    }

    #[test]
    fn a_sound_drive_passes_with_out_of_band_lines_zeroed() {
        let o = check_output(out(vec![1.0, 2.0, 3.0, 4.0, 5.0], 0.0), 5, (1, 4)).unwrap();
        assert_eq!(o.drive_psd, vec![0.0, 2.0, 3.0, 4.0, 0.0]);
    }

    #[test]
    fn each_malformed_output_is_named() {
        let band = (0, 3);
        assert_eq!(
            check_output(out(vec![1.0, 1.0], 0.0), 3, band).unwrap_err(),
            LawFaultReason::WrongLength
        );
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                check_output(out(vec![1.0, bad, 1.0], 0.0), 3, band).unwrap_err(),
                LawFaultReason::NonFinite
            );
        }
        assert_eq!(
            check_output(out(vec![1.0, -1e-30, 1.0], 0.0), 3, band).unwrap_err(),
            LawFaultReason::Negative
        );
        // A non-finite value out of band is still a broken law.
        assert_eq!(
            check_output(out(vec![1.0, 1.0, f64::NAN], 0.0), 3, (0, 2)).unwrap_err(),
            LawFaultReason::NonFinite
        );
        assert_eq!(
            check_output(out(vec![1.0, 1.0, 1.0], f64::NAN), 3, band).unwrap_err(),
            LawFaultReason::NonFiniteLevel
        );
    }
}
