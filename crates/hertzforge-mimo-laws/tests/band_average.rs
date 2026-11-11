//! The band-average law on the MIMO law boundary: what it declares, what it
//! reports, where its buzz cross terms come from, and that its law file runs as
//! the native law does.
//!
//! The law's own arithmetic is tested in `hertzforge-law-band-average-math`, and
//! against the Python law it is a port of by that crate's `parity` example. The
//! law-file tests need the law files built by `scripts/build_laws.sh`.

mod common;

use common::*;
use hertzforge_law_band_average_math::{BandAverage, Frame, Params};
use hertzforge_law_host::{DiagnosticKind, Param, ParamValue};
use hertzforge_mimo_laws::{BandAverageLaw, BuiltinMimoLaw, DiagKind, ParamVal};
use hertzforge_sdm::law::{DiagnosticData, MimoControlLaw, MimoInput};

const FILE: &str = "hertzforge_law_mimo_band_average";

/// Native against the law file: the bar the built-in laws are held to.
const PARITY: f64 = 1e-12;

fn synthetic(seed: u64) -> Synthetic {
    Synthetic::random(seed, 6, 4, 256)
}

fn scalar(run: &Run, l: usize, key: &str) -> f64 {
    match &run.outputs[l]
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .unwrap_or_else(|| panic!("{key} is reported"))
        .data
    {
        DiagnosticData::Scalar(x) => *x,
        other => panic!("{key} is {other:?}"),
    }
}

/// Every declared diagnostic is reported once each loop, in its declared layout,
/// finite; the threshold is set on the band's lines and nowhere else, and the
/// gain correction moves once the response has been measured.
#[test]
fn the_law_reports_the_diagnostics_it_declares() {
    let s = synthetic(21);
    let (lines, nc) = (s.setup.lines, s.setup.controls);
    let mut law = BandAverageLaw::with_params(&s.setup, &[]).expect("builds");
    assert_eq!(law.needs(), [MimoInput::LastDrive]);
    let run = closed_loop(&mut law, &s, 4);
    let declared = BandAverageLaw::spec().diagnostics;
    for (l, out) in run.outputs.iter().enumerate() {
        assert_eq!(out.diagnostics.len(), declared.len(), "loop {l}");
        for d in &declared {
            let got = out
                .diagnostics
                .iter()
                .find(|v| v.key == d.key)
                .unwrap_or_else(|| panic!("loop {l}: {} is reported", d.key));
            let values = match (&got.data, d.kind) {
                (DiagnosticData::Scalar(x), DiagKind::Scalar) => vec![*x],
                (DiagnosticData::PerLine(v), DiagKind::PerLine) => {
                    assert_eq!(v.len(), lines);
                    v.clone()
                }
                (DiagnosticData::PerControlLine(v), DiagKind::PerControlLine) => {
                    assert_eq!(v.len(), lines * nc);
                    v.clone()
                }
                other => panic!("loop {l}: {} is {other:?}", d.key),
            };
            assert!(values.iter().all(|x| x.is_finite()), "loop {l}: {}", d.key);
        }
    }
    let (lo, hi) = s.setup.band;
    let last = run.outputs.last().unwrap();
    let per_line = |key: &str| match &last.diagnostics.iter().find(|d| d.key == key).unwrap().data {
        DiagnosticData::PerLine(v) | DiagnosticData::PerControlLine(v) => v.clone(),
        other => panic!("{other:?}"),
    };
    let threshold = per_line("threshold");
    assert!(threshold[lo..hi].iter().all(|&k| k >= 1.0), "in band");
    assert!(threshold[..lo]
        .iter()
        .chain(&threshold[hi..])
        .all(|&k| k == 0.0));
    let gain = per_line("gain_db");
    assert!(
        gain[lo * nc..hi * nc].iter().any(|&g| g != 0.0),
        "the gain moved"
    );
    assert!(gain[..lo * nc].iter().all(|&g| g == 0.0));
    // The seed changes nothing, and nothing is left out or updated on a held FRF.
    assert_eq!(scalar(&run, 0, "correction_db"), 0.0);
    assert!(scalar(&run, 1, "correction_db") > 0.0);
    for key in ["excluded", "frf_updates", "held_bands", "capped_bands"] {
        assert_eq!(scalar(&run, 4, key), 0.0, "{key}");
    }
}

/// With the FRF update on, the law solves again from the FRF it is handed once
/// the estimate holds enough frames, and says how many times it has.
#[test]
fn accepted_frf_updates_are_counted() {
    let s = synthetic(22);
    let mut law = BandAverageLaw::with_params(
        &s.setup,
        &[
            ("use_tf_update", ParamVal::Toggle(true)),
            ("min_coherence", ParamVal::Number(0.5)),
        ],
    )
    .expect("builds");
    let run = closed_loop(&mut law, &s, 3);
    assert_eq!(scalar(&run, 0, "frf_updates"), 0.0);
    assert_eq!(scalar(&run, 3, "frf_updates"), 3.0);
    // Held to more frames than the estimate has, it never updates.
    let mut waiting = BandAverageLaw::with_params(
        &s.setup,
        &[
            ("use_tf_update", ParamVal::Toggle(true)),
            ("estimate_frames", ParamVal::Integer(1000)),
        ],
    )
    .expect("builds");
    let run = closed_loop(&mut waiting, &s, 3);
    assert_eq!(scalar(&run, 3, "frf_updates"), 0.0);
}

/// Buzz cross terms with no coherence floor are the Python law's `use_buzz`: the
/// buzz-test law's reference, fitted band by band, gives the drive the law's own
/// arithmetic gives from the identification response.
#[test]
fn buzz_cross_terms_at_no_floor_are_the_laws_own_buzz() {
    let s = synthetic(23);
    let (lines, nc, nd) = (s.setup.lines, s.setup.controls, s.setup.drives);
    let mut law = BandAverageLaw::with_params(
        &s.setup,
        &[
            ("cross_terms", ParamVal::Choice("buzz".into())),
            ("coherence_floor", ParamVal::Number(0.0)),
        ],
    )
    .expect("builds");
    let run = closed_loop(&mut law, &s, 0);
    let frequencies: Vec<f64> = (0..lines).map(|k| k as f64 * s.setup.df_hz).collect();
    let mut own = BandAverage::new(
        Params {
            df: s.setup.df_hz,
            drive_rms_limit: None,
            use_buzz: true,
            control_to_cross_terms: true,
            ..Params::default()
        },
        lines,
        nc,
        nd,
    );
    let drive = own
        .control(&Frame {
            frequencies: &frequencies,
            specification: Some(&s.setup.reference),
            transfer_function: s.frf.data(),
            sysid_response_cpsd: Some(&s.sysid_response),
            multiple_coherence: None,
            frames: 64,
            total_frames: 20,
            last_response_asd: None,
            last_output_cpsd: None,
        })
        .expect("solves");
    let mut worst: f64 = 0.0;
    for line in s.setup.band.0..s.setup.band.1 {
        let a = full(&run.outputs[0].drive, line);
        let b = &drive[line * nd * nd..(line + 1) * nd * nd];
        let scale = b.iter().map(|x| x.norm_sqr()).sum::<f64>().sqrt();
        let diff = a
            .iter()
            .zip(b)
            .map(|(x, y)| (x - y).norm_sqr())
            .sum::<f64>()
            .sqrt();
        worst = worst.max(diff / scale);
    }
    assert!(worst < 1e-8, "the two differ by {worst:e}");
    // With a floor, pairs under it are left out, and the drive is another.
    let mut floored = BandAverageLaw::with_params(
        &s.setup,
        &[
            ("cross_terms", ParamVal::Choice("buzz".into())),
            ("coherence_floor", ParamVal::Number(0.9)),
        ],
    )
    .expect("builds");
    let other = closed_loop(&mut floored, &s, 0);
    let moved = (s.setup.band.0..s.setup.band.1)
        .map(|line| line_rel(&run.outputs[0].drive, &other.outputs[0].drive, line))
        .fold(0.0f64, f64::max);
    assert!(moved > 1e-3, "a floor of 0.9 moved the drive by {moved:e}");
}

/// The drive RMS limit is off unless it is set: the core holds the ceilings. Set,
/// the law keeps every drive under it.
#[test]
fn the_rms_limit_is_off_until_it_is_set() {
    let s = synthetic(24);
    let rms = |drive: &hertzforge_sdm::HermitianStack| -> Vec<f64> {
        let mut power = vec![0.0; s.setup.drives];
        for line in s.setup.band.0..s.setup.band.1 {
            for (p, x) in power.iter_mut().zip(drive.diagonal(line)) {
                *p += x * s.setup.df_hz;
            }
        }
        power.iter().map(|p| p.sqrt()).collect()
    };
    let mut free = BandAverageLaw::with_params(&s.setup, &[]).expect("builds");
    let unlimited = rms(&closed_loop(&mut free, &s, 0).outputs[0].drive);
    let top = unlimited.iter().copied().fold(0.0f64, f64::max);
    let limit = 0.7 * top;
    let mut held =
        BandAverageLaw::with_params(&s.setup, &[("drive_rms_limit", ParamVal::Number(limit))])
            .expect("builds");
    let limited = rms(&closed_loop(&mut held, &s, 0).outputs[0].drive);
    assert!(top > limit);
    assert!(
        limited.iter().all(|&r| r <= limit * (1.0 + 1e-9)),
        "{limited:?} against {limit}"
    );
}

#[test]
fn the_law_file_declares_what_the_native_law_declares() {
    let spec = BandAverageLaw::spec();
    let law = load(FILE);
    let m = law.manifest();
    assert_eq!(m.law.name, spec.name);
    assert_eq!(m.law.version, spec.version);
    assert_eq!(m.law.author, spec.author);
    assert_eq!(m.shapes, spec.shapes);
    assert_eq!(m.needs, spec.needs);
    let keys: Vec<&str> = m.law.params.iter().map(|p| p.key.as_str()).collect();
    let want: Vec<&str> = spec.params.iter().map(|p| p.key).collect();
    assert_eq!(keys, want);
    let kinds: Vec<(String, DiagnosticKind)> = m
        .diagnostic_layouts
        .iter()
        .map(|l| (l.key.clone(), l.kind))
        .collect();
    let want: Vec<(String, DiagnosticKind)> = spec
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.key.to_string(),
                match d.kind {
                    DiagKind::Scalar => DiagnosticKind::Scalar,
                    DiagKind::PerLine => DiagnosticKind::PerLine,
                    DiagKind::PerControlLine => DiagnosticKind::PerControlLine,
                    DiagKind::PerDriveLine => DiagnosticKind::PerDriveLine,
                },
            )
        })
        .collect();
    assert_eq!(kinds, want);
}

/// A case's name, its parameters as the native law takes them and as the host does.
type Case = (&'static str, Vec<(&'static str, ParamVal)>, Vec<Param>);

/// The law file runs as the native law does, at its defaults and with the FRF
/// update, the reference's cross terms and a drive reduction; and a 1 percent
/// change of the gain correction's rate is seen.
#[test]
fn the_law_agrees_native_and_as_a_law_file() {
    let s = synthetic(25);
    let cases: [Case; 2] = [
        ("defaults", Vec::new(), Vec::new()),
        (
            "FRF update, cross terms, drive reduction",
            vec![
                ("use_tf_update", ParamVal::Toggle(true)),
                ("cross_terms", ParamVal::Choice("reference".into())),
                ("drive_reduction", ParamVal::Number(0.3)),
            ],
            vec![
                Param::new("use_tf_update", ParamValue::Toggle(true)),
                Param::new("cross_terms", ParamValue::Choice("reference".into())),
                Param::new("drive_reduction", ParamValue::Number(0.3)),
            ],
        ),
    ];
    for (name, given, params) in cases {
        let mut native = BandAverageLaw::with_params(&s.setup, &given).expect("builds");
        let mut file = wasm_law(FILE, &s.setup, &params);
        let p = parity(&mut native, &mut file, &s, 4);
        println!("band average, {name}: {p:?}");
        assert_eq!(p.loops, 5);
        assert!(p.drive_scale > 0.0);
        assert!(
            p.drive <= PARITY && p.diagnostics <= PARITY,
            "{name}: {p:?}"
        );
    }
    let mut native = BandAverageLaw::with_params(
        &s.setup,
        &[("gain_correction_rate", ParamVal::Number(0.25))],
    )
    .expect("builds");
    let mut file = wasm_law(
        FILE,
        &s.setup,
        &[Param::new(
            "gain_correction_rate",
            ParamValue::Number(0.2525),
        )],
    );
    let p = parity(&mut native, &mut file, &s, 3);
    assert!(p.drive > 1e3 * PARITY, "a 1% change was not seen: {p:?}");
}
