//! Builders and a seeded generator the MIMO Mixed Mode tests share.
#![allow(dead_code, clippy::needless_range_loop)]

use hertzforge_mimo_mixed::band::BandStatement;
use hertzforge_mimo_mixed::config::*;
use hertzforge_mixed::{BandWidth, PartSchedule, PartWindow, ToneAmplitude};
use hertzforge_random::service::config::LevelSchedule;
use hertzforge_reference::{Method, Provenance, Reference};
use hertzforge_sdm::law::Tolerances;
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

/// SplitMix64 with normals by the polar transform: deterministic, no dependency.
pub struct Rng {
    s: u64,
    spare: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng {
            s: seed ^ 0x9E37_79B9_7F4A_7C15,
            spare: None,
        }
    }

    pub fn u64(&mut self) -> u64 {
        self.s = self.s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1).
    pub fn uniform(&mut self) -> f64 {
        ((self.u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal.
    pub fn normal(&mut self) -> f64 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let (u, v) = (self.uniform(), self.uniform());
        let r = (-2.0 * u.ln()).sqrt();
        let t = std::f64::consts::TAU * v;
        self.spare = Some(r * t.sin());
        r * t.cos()
    }

    /// Circular complex normal of unit variance (`E|z|^2 = 1`).
    pub fn cnormal(&mut self) -> C64 {
        C64::new(self.normal(), self.normal()) * std::f64::consts::FRAC_1_SQRT_2
    }
}

/// A Hermitian matrix's lower Cholesky factor (positive definite input).
pub fn cholesky(a: &[Vec<C64>]) -> Vec<Vec<C64>> {
    let n = a.len();
    let mut l = vec![vec![C64::new(0.0, 0.0); n]; n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i][j];
            for k in 0..j {
                s -= l[i][k] * l[j][k].conj();
            }
            if i == j {
                l[i][j] = C64::new(s.re.max(0.0).sqrt(), 0.0);
            } else {
                l[i][j] = s / l[j][j].re;
            }
        }
    }
    l
}

/// A flat reference: every line of `[lo, hi)` holds `level * coh`, `coh`
/// the `n` by `n` coherence-and-phase matrix; zero outside the band.
pub fn reference(
    lines: usize,
    df: f64,
    band: (usize, usize),
    level: f64,
    coh: &[Vec<C64>],
) -> Reference {
    let n = coh.len();
    let mut sdm = HermitianStack::zeros(lines, n);
    for l in band.0..band.1 {
        for r in 0..n {
            for c in r..n {
                sdm.set(l, r, c, coh[r][c] * level);
            }
        }
    }
    Reference {
        sdm,
        df_hz: df,
        band,
        labels: (0..n).map(|i| format!("c{i}")).collect(),
        unit: "g".into(),
        tolerances: Tolerances::none(lines, n),
        response_transformation: None,
        drive_transformation: None,
        provenance: Provenance::new(Method::Breakpoints, "test"),
    }
}

/// An `n` by `n` coherence matrix with `gamma` (real) off the diagonal.
pub fn coherence(n: usize, gamma: f64) -> Vec<Vec<C64>> {
    (0..n)
        .map(|r| {
            (0..n)
                .map(|c| C64::new(if r == c { 1.0 } else { gamma }, 0.0))
                .collect()
        })
        .collect()
}

pub fn floor(id: &str) -> MimoPart {
    MimoPart {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body: MimoPartBody::Floor(MimoFloorPart::default()),
    }
}

pub fn fixed(hz: f64) -> PartSchedule {
    PartSchedule::Fixed {
        hz,
        for_s: None,
        for_cycles: None,
    }
}

pub fn tone(id: &str, schedule: PartSchedule, g_pk: f64) -> MimoPart {
    MimoPart {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body: MimoPartBody::Tone(MimoTonePart {
            schedule,
            amplitude: PerControl::All(ToneAmplitude::Constant { g_pk }),
            phases_stated: false,
            phase_deg: Vec::new(),
            alarm_db: PerControl::All(DEFAULT_TONE_ALARM_DB),
            abort_db: PerControl::All(DEFAULT_TONE_ABORT_DB),
        }),
    }
}

pub fn band(id: &str, schedule: PartSchedule, width_hz: f64, statement: BandStatement) -> MimoPart {
    MimoPart {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body: MimoPartBody::Narrowband(MimoNarrowbandPart {
            schedule,
            width: BandWidth::Hz { hz: width_hz },
            statement,
            alarm_db: PerControl::All(DEFAULT_BAND_ALARM_DB),
            abort_db: PerControl::All(DEFAULT_BAND_ABORT_DB),
        }),
    }
}

pub fn added(a: f64, coherence: f64) -> BandStatement {
    BandStatement::Added {
        level_g2_hz: PerControl::All(a),
        coherence,
    }
}

/// A configuration of `n` controls and `m` drives on a 1 Hz grid of 2048
/// lines, band 20 to 2000 Hz, floor 0.01 g^2/Hz with coherence `gamma`.
pub fn config(n: usize, m: usize, gamma: f64, parts: Vec<MimoPart>) -> MimoMixedConfig {
    MimoMixedConfig {
        estimation: Default::default(),
        controls: (0..n as u32).collect(),
        drives: (0..m as u32)
            .map(|j| MimoDrive::new(100 + j, 2.0))
            .collect(),
        limits: Vec::new(),
        reference: reference(2048, 1.0, (20, 2001), 0.01, &coherence(n, gamma)),
        parts,
        exclusion_hz: DEFAULT_EXCLUSION_HZ,
        tracking_hz: DEFAULT_TRACKING_HZ,
        schedule: LevelSchedule::auto_default(),
        full_level_duration_s: Some(60.0),
        level_ramp_s: DEFAULT_LEVEL_RAMP_S,
        shutdown_ramp_s: 0.0,
        part_ramp_s: 3.0,
        floor_settle_s: 30.0,
        soft_start_db: 9.0,
        soft_start_relax_db: 6.0,
        identification: None,
        gates: Default::default(),
        caps: Default::default(),
        protections: Default::default(),
        frf_mode: hertzforge_mimo_random::config::FrfMode::Hold,
        crest_factor: 3.0,
        drive_clip_sigma: None,
        article_mass_lb: 0.0,
        capture: Default::default(),
        law: MimoMixedLawChoice::default(),
        control_tick_s: None,
        metadata: Vec::new(),
        seed: 1,
    }
}

/// A static model `h` (rows of `m`) on every line.
pub fn static_model(lines: usize, h: &[Vec<C64>]) -> MatrixStack {
    let (n, m) = (h.len(), h[0].len());
    let mut s = MatrixStack::zeros(lines, n, m);
    for l in 0..lines {
        for r in 0..n {
            for c in 0..m {
                s.set(l, r, c, h[r][c]);
            }
        }
    }
    s
}

pub fn c(re: f64, im: f64) -> C64 {
    C64::new(re, im)
}
