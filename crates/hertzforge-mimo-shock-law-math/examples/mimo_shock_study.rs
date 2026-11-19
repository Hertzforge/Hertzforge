//! The MIMO Shock comparison study: each built-in law beside its published
//! baseline, on the exact reference plants, over the ladder (-12, -9, -6, -3
//! dB, then ten pulses at 0 dB). Fixed seeds, no file output; it prints
//! tables.
//!
//! ```text
//! cargo run --release -p hertzforge-mimo-shock-law-math --example mimo_shock_study
//! ```
//!
//! Every plant is identified exactly at -20 dB. A plant's modes drop by
//! `delta` and its damping grows by `1 + kappa` at full level, in proportion to
//! the pulse's amplitude scale. Columns: the worst-point SRS error (dB, every
//! control and natural frequency) at each pulse; the waveform error and the
//! drive growth (full-level drive rms over the first pulse's) at the last
//! pulse and at their worst; the delays the law built the last pulse for.

use hertzforge_mimo_shock_law_math::check::{drive_side, reachable_fraction, shape_jacobian};
use hertzforge_mimo_shock_law_math::delay::LeastEffort;
use hertzforge_mimo_shock_law_math::inverse::{InverseLaw, InverseParams};
use hertzforge_mimo_shock_law_math::model::regularized_inverse;
use hertzforge_mimo_shock_law_math::paired::{PairedLaw, PairedParams};
use hertzforge_mimo_shock_law_math::reference_plant::{
    run_ladder, srs_setup, ModalPlant, PulseRow, FS_HZ, LADDER_DB, RECORD_LEN,
};
use hertzforge_mimo_shock_law_math::shape::{Anchor, SeedTiming, ShapeLaw, ShapeParams};
use hertzforge_mimo_shock_law_math::spectrum::rows_rfft;
use hertzforge_mimo_shock_law_math::{LawSteps, Setup};

const ID_SCALE: f64 = 0.1;
const SEED: u64 = 20_261_111;

#[derive(Clone, Copy)]
enum Law {
    InverseFrozen,
    InverseWatchdog,
    InverseRefreshHalf,
    InverseRefreshFull,
    InverseAlign,
    ShapeFrozenTiming,
    ShapeDefault,
    ShapeMeasured,
    ShapeLeastEffort,
    Paired,
}

impl Law {
    const ALL: [Law; 10] = [
        Law::InverseFrozen,
        Law::InverseWatchdog,
        Law::InverseRefreshHalf,
        Law::InverseRefreshFull,
        Law::InverseAlign,
        Law::ShapeFrozenTiming,
        Law::ShapeDefault,
        Law::ShapeMeasured,
        Law::ShapeLeastEffort,
        Law::Paired,
    ];

    fn label(self) -> &'static str {
        match self {
            Law::InverseFrozen => "inverse g1, no watchdog [baseline]",
            Law::InverseWatchdog => "inverse g1, watchdog (default)",
            Law::InverseRefreshHalf => "inverse g1, refresh mu 0.5",
            Law::InverseRefreshFull => "inverse g1, refresh mu 1",
            Law::InverseAlign => "inverse g1, watchdog, align",
            Law::ShapeFrozenTiming => "shape, no delay est. [baseline]",
            Law::ShapeDefault => "shape (default)",
            Law::ShapeMeasured => "shape, measured anchor + refresh",
            Law::ShapeLeastEffort => "shape, least-effort seed",
            Law::Paired => "paired",
        }
    }

    fn build(self, setup: &Setup) -> Box<dyn LawSteps> {
        let inv = |p: InverseParams| -> Box<dyn LawSteps> {
            Box::new(InverseLaw::new(setup.clone(), p).expect("inverse"))
        };
        let shp = |p: ShapeParams| -> Box<dyn LawSteps> {
            Box::new(ShapeLaw::new(setup.clone(), p).expect("shape"))
        };
        match self {
            Law::InverseFrozen => inv(InverseParams {
                watchdog: false,
                ..Default::default()
            }),
            Law::InverseWatchdog => inv(InverseParams::default()),
            Law::InverseRefreshHalf => inv(InverseParams {
                watchdog: false,
                refresh: true,
                refresh_mu: 0.5,
                ..Default::default()
            }),
            Law::InverseRefreshFull => inv(InverseParams {
                watchdog: false,
                refresh: true,
                refresh_mu: 1.0,
                ..Default::default()
            }),
            Law::InverseAlign => inv(InverseParams {
                align: true,
                ..Default::default()
            }),
            Law::ShapeFrozenTiming => shp(ShapeParams {
                delay_relax: 0.0,
                ..Default::default()
            }),
            Law::ShapeDefault => shp(ShapeParams::default()),
            Law::ShapeMeasured => shp(ShapeParams {
                anchor: Anchor::Measured,
                refresh: true,
                refresh_mu: 1.0,
                ..Default::default()
            }),
            Law::ShapeLeastEffort => shp(ShapeParams {
                seed_timing: SeedTiming::LeastEffort(LeastEffort::default()),
                ..Default::default()
            }),
            Law::Paired => Box::new(
                PairedLaw::new(
                    setup.clone(),
                    PairedParams::diagonal(setup.n_controls, setup.n_drives),
                )
                .expect("paired"),
            ),
        }
    }
}

type MakePlant = fn(f64, f64) -> ModalPlant;

const SIZES: [(&str, MakePlant); 3] = [
    ("1 x 1", ModalPlant::siso),
    ("2 x 3 (2 drives, 3 controls)", ModalPlant::two_by_three),
    ("4 x 4", ModalPlant::four_by_four),
];

const CASES: [(f64, f64, &str); 4] = [
    (0.0, 0.0, "linear"),
    (0.05, 1.0, "5% drop, damping x2"),
    (0.05, 0.0, "5% drop, damping fixed"),
    (0.08, 0.0, "8% drop, damping fixed"),
];

fn growth_at(rows: &[PulseRow], k: usize) -> f64 {
    rows[k].drive_rms / rows[0].drive_rms
}

fn delays_ms(d: &Option<Vec<f64>>) -> String {
    match d {
        Some(v) => v
            .iter()
            .map(|x| format!("{:+.3}", x * 1e3))
            .collect::<Vec<_>>()
            .join(" "),
        None => "-".into(),
    }
}

fn print_header(n: usize) {
    print!("  {:36}", "law");
    for k in 1..=n {
        print!(" {:>5}", format!("p{k}"));
    }
    println!(
        "  {:>8} {:>8} {:>7} {:>7}  delays (ms)",
        "wave", "wave max", "grow", "grow mx"
    );
}

fn print_row(label: &str, rows: &[PulseRow]) {
    print!("  {:36}", label);
    for r in rows {
        print!(" {:5.1}", r.worst_srs_db.min(999.9));
    }
    let last = rows.len() - 1;
    let g_max = (0..rows.len())
        .map(|k| growth_at(rows, k))
        .fold(0.0, f64::max);
    let w_max = rows.iter().map(|r| r.waveform_error).fold(0.0, f64::max);
    println!(
        "  {:8.3} {:8.3} {:7.2} {:7.2}  {}",
        rows[last].waveform_error.min(9999.0),
        w_max.min(9999.0),
        growth_at(rows, last).min(9999.0),
        g_max.min(9999.0),
        delays_ms(&rows[last].delays_s)
    );
}

fn check_line(plant: &ModalPlant, setup: &Setup) {
    let id = plant.frf(ID_SCALE, FS_HZ, RECORD_LEN);
    let mask = vec![true; id.lines()];
    let g = regularized_inverse(&id, &mask, 1e-3).expect("inverse");
    let full = plant.frf(1.0, FS_HZ, RECORD_LEN);
    let d = drive_side(&g, &full, &mask, 1.0).expect("check");
    let df = FS_HZ / RECORD_LEN as f64;
    let unstable = d.radius.iter().filter(|&&r| r > 1.0).count();
    let r = rows_rfft(&setup.reference, 1.0).expect("spectra");
    let reach = reachable_fraction(&full, &mask, &r).expect("reach");
    println!(
        "  check, inverse from -20 dB against full level, gain 1: worst drive-side radius {:.2} at {:.0} Hz; \
         {} lines above 1; least safe gain {:.2} at {:.0} Hz; reachable fraction per control {}",
        d.worst.0,
        d.worst.1 as f64 * df,
        unstable,
        d.least_safe.0,
        d.least_safe.1 as f64 * df,
        reach
            .iter()
            .map(|v| format!("{v:.3}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    // The shape Jacobian at the seed (gains one, zero delays), on the lines of
    // the SRS band.
    let band: Vec<bool> = (0..id.lines())
        .map(|k| (100.0..=2000.0).contains(&(k as f64 * df)))
        .collect();
    let j = shape_jacobian(&g, &full, &band, &r).expect("jacobian");
    println!(
        "  shape Jacobian at the seed, 100 to 2000 Hz: {} lines; {} not diagonally dominant; \
         {} with an eigenvalue of nonpositive real part; least safe weight {:.2}",
        j.lines, j.lines_without_dominance, j.lines_unstable, j.least_safe_weight
    );
}

fn main() {
    println!("MIMO Shock comparison study");
    println!(
        "fs {FS_HZ} Hz, {RECORD_LEN} samples; identification exact at -20 dB; ladder {:?} dB; \
         seed {SEED}; no noise unless stated",
        LADDER_DB
    );
    println!("cells: worst-point SRS error in dB at each pulse");

    for (size, make) in SIZES {
        for (delta, kappa, case) in CASES {
            let plant = make(delta, kappa);
            let setup = srs_setup(&plant, ID_SCALE);
            println!();
            println!("== {size}, {case} (delta {delta}, kappa {kappa})");
            check_line(&plant, &setup);
            print_header(LADDER_DB.len());
            for law in Law::ALL {
                let mut l = law.build(&setup);
                let rows =
                    run_ladder(l.as_mut(), &setup, &plant, &LADDER_DB, 0.0, SEED).expect("run");
                print_row(law.label(), &rows);
            }
        }
    }

    // Longer runs: does the bounded behaviour last?
    let mut long = LADDER_DB.to_vec();
    long.extend(std::iter::repeat_n(0.0, 26));
    println!();
    println!(
        "== Longer runs: 40 pulses (the ladder, then 36 at 0 dB); cells at pulses 10, 14, 20, 30, 40"
    );
    println!(
        "  {:52} {:>26} {:>34} {:>30}",
        "plant / law", "worst SRS dB", "drive growth", "waveform error"
    );
    for (name, plant) in [
        ("1 x 1, 5% fixed", ModalPlant::siso(0.05, 0.0)),
        ("1 x 1, 8% fixed", ModalPlant::siso(0.08, 0.0)),
        ("2 x 3, 8% fixed", ModalPlant::two_by_three(0.08, 0.0)),
        ("4 x 4, 8% fixed", ModalPlant::four_by_four(0.08, 0.0)),
    ] {
        let setup = srs_setup(&plant, ID_SCALE);
        for law in [
            Law::InverseFrozen,
            Law::InverseWatchdog,
            Law::InverseRefreshFull,
            Law::ShapeDefault,
            Law::ShapeMeasured,
        ] {
            let mut l = law.build(&setup);
            let rows = run_ladder(l.as_mut(), &setup, &plant, &long, 0.0, SEED).expect("run");
            let at = [9usize, 13, 19, 29, 39];
            let fmt = |f: &dyn Fn(usize) -> f64, p: usize| -> String {
                at.iter()
                    .map(|&k| format!("{:.p$}", f(k).min(9999.0)))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            println!(
                "  {:52} {:>26} {:>34} {:>30}",
                format!("{name}: {}", law.label()),
                fmt(&|k| rows[k].worst_srs_db, 1),
                fmt(&|k| growth_at(&rows, k), 2),
                fmt(&|k| rows[k].waveform_error, 2),
            );
        }
    }

    // Measurement noise on the harsh plants.
    println!();
    println!("== 1% measurement noise (std 1% of the reference rms, absolute), seed {SEED}");
    for (name, plant) in [
        ("1 x 1, 5% fixed", ModalPlant::siso(0.05, 0.0)),
        ("2 x 3, 8% fixed", ModalPlant::two_by_three(0.08, 0.0)),
        ("4 x 4, 8% fixed", ModalPlant::four_by_four(0.08, 0.0)),
    ] {
        let setup = srs_setup(&plant, ID_SCALE);
        println!("  {name}");
        print_header(LADDER_DB.len());
        for law in [
            Law::InverseWatchdog,
            Law::InverseRefreshFull,
            Law::ShapeDefault,
            Law::ShapeMeasured,
        ] {
            let mut l = law.build(&setup);
            let rows = run_ladder(l.as_mut(), &setup, &plant, &LADDER_DB, 0.01, SEED).expect("run");
            print_row(law.label(), &rows);
        }
    }

    // Seed timing: the first pulse with zero delays and with least-effort
    // delays at three weights on the drive energy.
    println!();
    println!("== Seed timing: first pulse (-12 dB, normalized to full level)");
    println!(
        "  {:18} {:26} {:>9} {:>22} {:>10}  delays (ms)",
        "plant", "seed timing", "worst dB", "per control dB", "drive rms"
    );
    for (name, plant) in [
        ("2 x 3, linear", ModalPlant::two_by_three(0.0, 0.0)),
        ("2 x 3, 8% fixed", ModalPlant::two_by_three(0.08, 0.0)),
        ("4 x 4, linear", ModalPlant::four_by_four(0.0, 0.0)),
    ] {
        let setup = srs_setup(&plant, ID_SCALE);
        let le = |lambda: f64| {
            SeedTiming::LeastEffort(LeastEffort {
                lambda,
                ..Default::default()
            })
        };
        for (label, timing) in [
            ("zero", SeedTiming::Zero),
            ("least effort, lambda 0", le(0.0)),
            ("least effort, lambda 0.1", le(0.1)),
            ("least effort, lambda 1", le(1.0)),
        ] {
            let params = ShapeParams {
                seed_timing: timing,
                ..Default::default()
            };
            let mut l = ShapeLaw::new(setup.clone(), params).expect("shape");
            let rows = run_ladder(&mut l, &setup, &plant, &LADDER_DB[..1], 0.0, SEED).expect("run");
            let r = &rows[0];
            println!(
                "  {:18} {:26} {:9.2} {:>22} {:10.3}  {}",
                name,
                label,
                r.worst_srs_db,
                r.srs_db_per_control
                    .iter()
                    .map(|v| format!("{v:.2}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                r.drive_rms,
                delays_ms(&r.delays_s)
            );
        }
    }
}
