//! The convention, first: a tone played through the engine on
//! `sine-linear-4x4` gives tracked amplitudes equal to the identification's
//! response times the drive, in amplitude and phase, and the identification
//! equals the structure's exact small-signal response.

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::law::{LawDescriptor, LawManifest, LawSetup, LawValues};
use hertzforge_mimo_sine::*;
use std::sync::{Arc, Mutex};

/// A law that plays a fixed drive and never changes it.
struct Fixed(Vec<C64>);

impl MimoSineLaw for Fixed {
    fn name(&self) -> &str {
        "fixed"
    }
    fn engage(
        &mut self,
        _: &TickInputs<'_>,
        out: &mut TickOutputs,
        _: &mut LawValues,
    ) -> Result<(), LawFault> {
        out.drive.copy_from_slice(&self.0);
        Ok(())
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        out: &mut TickOutputs,
        _: &mut LawValues,
    ) -> Result<(), LawFault> {
        out.drive.copy_from_slice(x.drive_in_force);
        Ok(())
    }
}

/// Builds a [`Fixed`] law playing `scale` times the identification's inverse
/// drive for the reference at the dwell frequency.
struct FixedFactory {
    scale: f64,
    built: Mutex<Option<Vec<C64>>>,
}

impl MimoSineLawFactory for FixedFactory {
    fn build(&self, s: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        let f = s.band_lo_hz;
        let k = s
            .identification
            .frequency_hz
            .iter()
            .position(|g| (g - f).abs() < 1e-9)
            .expect("on a line");
        let (n, m) = (s.controls, s.drives);
        let h = s.identification.response.line(k);
        let r: Vec<C64> = (0..n).map(|c| s.reference[c * s.lines() + k]).collect();
        let (z, _) = hertzforge_mimo_sine_law_math::linalg::reg_pinv(h, n, m, 0.0).unwrap();
        let mut d = vec![C64::new(0.0, 0.0); m];
        hertzforge_mimo_sine_law_math::linalg::mat_vec(&z, m, n, &r, &mut d);
        d.iter_mut().for_each(|v| *v *= self.scale);
        *self.built.lock().unwrap() = Some(d.clone());
        Ok(Box::new(Fixed(d)))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "fixed".into(),
            name: "fixed".into(),
            version: "0".into(),
            sha256: None,
            params: vec![],
            diagnostics: vec![],
            live: vec![],
        }
    }
    fn manifest(&self) -> LawManifest {
        LawManifest {
            shapes: vec![
                MimoShape::Square,
                MimoShape::MoreControlsThanDrives,
                MimoShape::FewerControlsThanDrives,
            ],
            test_types: TestType::ALL.to_vec(),
            amplitude_only: true,
            values: vec![],
        }
    }
}

#[test]
fn mimo_sine_convention_tracked_controls_equal_the_identification_times_the_drive() {
    let s = SineStructure::Linear4x4;
    let f = 120.0;
    let factory = Arc::new(FixedFactory {
        scale: 0.7,
        built: Mutex::new(None),
    });
    let svc = engaged(s, setup(s, dwell(f, 4.0), "fixed"), factory.clone(), 0.0);

    // The identification against the exact small-signal response: the same
    // within 1e-4 of each line's largest element, after a pure delay of a
    // whole number of samples (the station's own buffering, which the
    // identification and the trackers both take out by the measured lag).
    let rep = svc.identification().unwrap().clone();
    let id = &rep.identification;
    let model = s.build().model;
    let exact = model.analytic_frf_at(&id.frequency_hz, FS);
    let nc = s.n_controls();
    let m = s.dims().1;
    let err_for = |delta: f64| {
        let mut worst = 0.0f64;
        for (k, fk) in id.frequency_hz.iter().enumerate() {
            let rot = C64::from_polar(1.0, std::f64::consts::TAU * fk * delta / FS);
            let big = (0..nc * m)
                .map(|e| id.response.line(k)[e].norm())
                .fold(0.0, f64::max);
            for c in 0..nc {
                for d in 0..m {
                    let e = (id.response.get(k, c, d) - exact.get(k, c, d) * rot).norm() / big;
                    worst = worst.max(e);
                }
            }
        }
        worst
    };
    let (delta, worst) = (-400..=400)
        .map(|d| (d, err_for(d as f64)))
        .fold((0, f64::INFINITY), |a, b| if b.1 < a.1 { b } else { a });
    println!("identification: lag {} samples, delay {delta} samples, worst relative error {worst:.2e}, level {:?} dB", rep.lag_frames, rep.level_db);
    assert!(
        worst < 1e-4,
        "the identification is off the exact response by {worst:.2e}"
    );
    assert!(
        rep.level_db.is_some_and(|l| (l + 20.0).abs() < 3.0),
        "level {:?}",
        rep.level_db
    );

    // The tone: after the drive's ramp and the trackers' settle, C = H D.
    let run = run(svc, 6.0);
    assert_eq!(
        run.outcome,
        Some(records::Outcome::Completed),
        "{:?}",
        run.events
    );
    let d = factory.built.lock().unwrap().clone().unwrap();
    assert!(d.iter().all(|v| v.norm() > 1e-6), "every drive plays");
    let k = id
        .frequency_hz
        .iter()
        .position(|g| (g - f).abs() < 1e-9)
        .unwrap();
    let mut hd = vec![C64::new(0.0, 0.0); nc];
    for c in 0..nc {
        for j in 0..m {
            hd[c] += id.response.get(k, c, j) * d[j];
        }
    }
    let late: Vec<_> = run
        .ticks
        .iter()
        .filter(|t| t.time_s > 3.0 && t.time_s < 3.9)
        .collect();
    assert!(!late.is_empty());
    let mut worst = 0.0f64;
    for t in &late {
        for c in 0..nc {
            assert_eq!(t.drives()[0], d[0]);
            let e = (t.controls()[c] - hd[c]).norm() / hd[c].norm();
            worst = worst.max(e);
        }
    }
    println!("tracked against H D: worst relative error {worst:.2e}");
    assert!(worst < 1e-3, "C differs from H D by {worst:.2e}");
}
