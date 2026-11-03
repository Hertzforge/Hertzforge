//! What the MIMO law tests share: a seeded generator, small complex matrix
//! helpers, a synthetic plant (a random Hermitian reference, an FRF stack, an
//! identification set, and a response `H G H^H` with a small perturbation),
//! the law files built by `scripts/build_laws.sh`, and the parity harness that
//! runs one law natively and as a law file over the same synthetic run.

#![allow(dead_code)]

use hertzforge_law_host::{HostConfig, LawHost, LoadedMimoLaw, Param, WasmMimoLaw};
use hertzforge_sdm::law::{
    DiagnosticData, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup,
    Tolerances,
};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::path::PathBuf;
use std::time::Duration;

// -------------------------------------------------------------------------
// A seeded generator (splitmix64), so every run is the same run
// -------------------------------------------------------------------------

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform on [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal (Box-Muller).
    pub fn normal(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    /// Circular complex normal with unit variance.
    pub fn cnormal(&mut self) -> C64 {
        C64::new(self.normal(), self.normal()) * std::f64::consts::FRAC_1_SQRT_2
    }
}

// -------------------------------------------------------------------------
// Per-line complex matrices (row-major slices)
// -------------------------------------------------------------------------

/// `a` (r x k) times `b` (k x c).
pub fn matmul(a: &[C64], b: &[C64], r: usize, k: usize, c: usize) -> Vec<C64> {
    let mut out = vec![C64::new(0.0, 0.0); r * c];
    for i in 0..r {
        for j in 0..c {
            let mut s = C64::new(0.0, 0.0);
            for t in 0..k {
                s += a[i * k + t] * b[t * c + j];
            }
            out[i * c + j] = s;
        }
    }
    out
}

/// The conjugate transpose of `a` (r x c).
pub fn ctrans(a: &[C64], r: usize, c: usize) -> Vec<C64> {
    let mut out = vec![C64::new(0.0, 0.0); r * c];
    for i in 0..r {
        for j in 0..c {
            out[j * r + i] = a[i * c + j].conj();
        }
    }
    out
}

/// One line of a Hermitian stack in full.
pub fn full(h: &HermitianStack, line: usize) -> Vec<C64> {
    let n = h.n();
    let mut out = vec![C64::new(0.0, 0.0); n * n];
    for r in 0..n {
        for c in 0..n {
            out[r * n + c] = h.get(line, r, c);
        }
    }
    out
}

/// `H G H^H` per line: `h` is nc x nd, `g` nd x nd, the result nc x nc.
pub fn congruence(h: &MatrixStack, g: &HermitianStack) -> HermitianStack {
    let (lines, nc, nd) = h.dims();
    let mut out = HermitianStack::zeros(lines, nc);
    for line in 0..lines {
        let hg = matmul(h.line(line), &full(g, line), nc, nd, nd);
        let y = matmul(&hg, &ctrans(h.line(line), nc, nd), nc, nd, nc);
        for r in 0..nc {
            for c in r..nc {
                // Average the two triangles: H G H^H is Hermitian up to
                // rounding.
                out.set(line, r, c, (y[r * nc + c] + y[c * nc + r].conj()) * 0.5);
            }
        }
    }
    out
}

/// The cross spectrum `H G` (nc x nd per line) a response to drive `g`
/// through `h` has with the drive, in the `E[Y X^H]` convention.
pub fn cross(h: &MatrixStack, g: &HermitianStack) -> MatrixStack {
    let (lines, nc, nd) = h.dims();
    let mut data = Vec::with_capacity(lines * nc * nd);
    for line in 0..lines {
        data.extend(matmul(h.line(line), &full(g, line), nc, nd, nd));
    }
    MatrixStack::from_vec(lines, nc, nd, data).expect("dims")
}

// -------------------------------------------------------------------------
// The synthetic plant
// -------------------------------------------------------------------------

/// A synthetic run: the reference, the true plant and the estimate the law is
/// handed, the identification set, and a response model.
pub struct Synthetic {
    pub setup: MimoSetup,
    /// The true plant, controls by drives.
    pub plant: MatrixStack,
    /// The estimate handed to the law.
    pub frf: MatrixStack,
    pub coherence: Vec<f64>,
    pub sysid_drive: HermitianStack,
    pub sysid_response: HermitianStack,
    pub sysid_noise: HermitianStack,
    /// Relative size of the per-loop response perturbation (a congruence by
    /// `diag(1 + eps * n_i)`, which keeps the response Hermitian and positive
    /// semi-definite).
    pub eps: f64,
    pub seed: u64,
}

impl Synthetic {
    /// A random run on `lines` lines, `nc` controls and `nd` drives: a random
    /// positive semi-definite reference of full rank in band (zero outside),
    /// a random FRF stack, the estimate equal to the plant, and a 1 percent
    /// response perturbation.
    pub fn random(seed: u64, nc: usize, nd: usize, lines: usize) -> Self {
        let mut rng = Rng::new(seed);
        let band = (lines / 8, lines - lines / 8);
        let mut plant = MatrixStack::zeros(lines, nc, nd);
        for v in plant.data_mut() {
            *v = rng.cnormal();
        }
        Self::with_plant(seed, plant, band, 0.01)
    }

    /// A run on the given plant and band.
    pub fn with_plant(seed: u64, plant: MatrixStack, band: (usize, usize), eps: f64) -> Self {
        let (lines, nc, nd) = plant.dims();
        let mut rng = Rng::new(seed.wrapping_mul(31).wrapping_add(7));
        let mut reference = HermitianStack::zeros(lines, nc);
        for line in band.0..band.1 {
            let a: Vec<C64> = (0..nc * nc).map(|_| rng.cnormal()).collect();
            let g = matmul(&a, &ctrans(&a, nc, nc), nc, nc, nc);
            let shape = 1e-2 * (1.0 + 0.5 * (0.3 * line as f64).sin());
            for r in 0..nc {
                for c in r..nc {
                    reference.set(line, r, c, g[r * nc + c] * (shape / nc as f64));
                }
            }
        }
        let mut sysid_drive = HermitianStack::zeros(lines, nd);
        for line in band.0..band.1 {
            for d in 0..nd {
                sysid_drive.set(line, d, d, C64::new(1e-3, 0.0));
            }
        }
        let sysid_response = congruence(&plant, &sysid_drive);
        let mut sysid_noise = HermitianStack::zeros(lines, nc);
        for line in 0..lines {
            for c in 0..nc {
                sysid_noise.set(line, c, c, C64::new(1e-9, 0.0));
            }
        }
        Synthetic {
            setup: MimoSetup {
                controls: nc,
                drives: nd,
                lines,
                df_hz: 2.5,
                band,
                reference,
                tolerances: Tolerances::none(lines, nc),
            },
            frf: plant.clone(),
            plant,
            coherence: vec![0.98; lines * nc],
            sysid_drive,
            sysid_response,
            sysid_noise,
            eps,
            seed,
        }
    }

    /// The measured response to `drive` on loop `l`: `H G H^H` through the
    /// true plant, perturbed by a seeded congruence of relative size `eps`.
    pub fn respond(&self, drive: &HermitianStack, l: u64) -> HermitianStack {
        let mut y = congruence(&self.plant, drive);
        let mut rng = Rng::new(self.seed ^ (l.wrapping_mul(0x5851_f42d_4c95_7f2d)));
        let (lines, nc) = (y.lines(), y.n());
        let scales: Vec<f64> = (0..lines * nc)
            .map(|_| (1.0 + self.eps * rng.normal()).powi(2))
            .collect();
        y.diagonal_congruence(&scales).expect("dims");
        y
    }

    /// The inputs of loop `l` with `response`, carrying the optional inputs
    /// in `needs` only, as the controller does. `last_drive` is the drive
    /// played last loop.
    pub fn inputs<'a>(
        &'a self,
        response: &'a HermitianStack,
        last_drive: &'a HermitianStack,
        csd: &'a MatrixStack,
        l: u64,
        needs: &[MimoInput],
    ) -> MimoLoopInputs<'a> {
        let has = |n: MimoInput| needs.contains(&n);
        MimoLoopInputs {
            reference: &self.setup.reference,
            tolerances: &self.setup.tolerances,
            response,
            response_dof: 128.0,
            frf: &self.frf,
            multiple_coherence: &self.coherence,
            df_hz: self.setup.df_hz,
            band: self.setup.band,
            loop_index: l,
            last_drive: has(MimoInput::LastDrive).then_some(last_drive),
            response_drive_csd: has(MimoInput::ResponseDriveCsd).then_some(csd),
            sysid_response: has(MimoInput::SysidResponse).then_some(&self.sysid_response),
            sysid_drive: has(MimoInput::SysidDrive).then_some(&self.sysid_drive),
            sysid_noise: has(MimoInput::SysidNoise).then_some(&self.sysid_noise),
        }
    }
}

// -------------------------------------------------------------------------
// Law files
// -------------------------------------------------------------------------

/// The bytes of `<name>.wasm` from `<target>/wasm32-wasip2/release/`, built by
/// scripts/build_laws.sh. Fails, naming the script, when it is not built.
pub fn law_bytes(name: &str) -> Vec<u8> {
    let file = format!("{name}.wasm");
    let mut tried: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("HERTZFORGE_LAW_WASM_DIR") {
        tried.push(PathBuf::from(dir).join(&file));
    } else {
        let exe = std::env::current_exe().expect("the test binary knows its own path");
        for dir in exe.ancestors().skip(1).take(4) {
            tried.push(dir.join("wasm32-wasip2").join("release").join(&file));
        }
    }
    for path in &tried {
        if path.is_file() {
            return std::fs::read(path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        }
    }
    panic!(
        "the law file {file} is not built. Run `bash scripts/build_laws.sh`; it builds every \
         guest law into <target>/wasm32-wasip2/release/. Looked in: {tried:#?}"
    );
}

pub const BUDGET: Duration = Duration::from_secs(20);

pub fn load(name: &str) -> LoadedMimoLaw {
    LawHost::new(HostConfig::default())
        .expect("the host starts")
        .load_mimo(&law_bytes(name))
        .unwrap_or_else(|e| panic!("{name} loads: {e}"))
}

/// The law file `name` built for `setup` with `params`, as the controller's
/// MIMO law.
pub fn wasm_law(name: &str, setup: &MimoSetup, params: &[Param]) -> WasmMimoLaw {
    WasmMimoLaw::build(&load(name), setup, params, BUDGET)
        .unwrap_or_else(|e| panic!("{name} builds: {e}"))
}

// -------------------------------------------------------------------------
// Parity
// -------------------------------------------------------------------------

/// Relative Frobenius difference of one line: `||a - b|| / max(||a||, ||b||)`,
/// 0 when both are zero.
pub fn line_rel(a: &HermitianStack, b: &HermitianStack, line: usize) -> f64 {
    let (mut num, mut na, mut nb) = (0.0, 0.0, 0.0);
    for (x, y) in a.line(line).iter().zip(b.line(line)) {
        num += (x - y).norm_sqr();
        na += x.norm_sqr();
        nb += y.norm_sqr();
    }
    let den = na.max(nb);
    if den == 0.0 {
        0.0
    } else {
        (num / den).sqrt()
    }
}

/// The worst relative difference of two diagnostics lists: per value
/// `|a - b| / max(|a|, |b|, 1e-300 + scale * 1e-12)` with `scale` the largest
/// magnitude of that diagnostic, so values that are zero up to rounding do
/// not count as differences. A key missing on one side, or a different
/// layout, is infinite.
pub fn diag_rel(a: &MimoLoopOutputs, b: &MimoLoopOutputs) -> f64 {
    let values = |d: &DiagnosticData| -> Vec<f64> {
        match d {
            DiagnosticData::Scalar(x) => vec![*x],
            DiagnosticData::PerLine(v)
            | DiagnosticData::PerControlLine(v)
            | DiagnosticData::PerDriveLine(v) => v.clone(),
        }
    };
    if a.diagnostics.len() != b.diagnostics.len() {
        return f64::INFINITY;
    }
    let mut worst: f64 = 0.0;
    for da in &a.diagnostics {
        let Some(db) = b.diagnostics.iter().find(|d| d.key == da.key) else {
            return f64::INFINITY;
        };
        if std::mem::discriminant(&da.data) != std::mem::discriminant(&db.data) {
            return f64::INFINITY;
        }
        let (va, vb) = (values(&da.data), values(&db.data));
        if va.len() != vb.len() {
            return f64::INFINITY;
        }
        let scale = va.iter().chain(&vb).fold(0.0f64, |m, x| m.max(x.abs()));
        for (x, y) in va.iter().zip(&vb) {
            let den = x.abs().max(y.abs()).max(1e-300 + scale * 1e-12);
            worst = worst.max((x - y).abs() / den);
        }
    }
    worst
}

/// What one parity run measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parity {
    /// Loops compared, the seed included.
    pub loops: usize,
    /// The worst relative line difference of the drives.
    pub drive: f64,
    /// The worst relative difference of the diagnostics.
    pub diagnostics: f64,
    /// The largest drive line norm seen, so a comparison of zeros is visible.
    pub drive_scale: f64,
}

/// Run `native` and `file` side by side over `loops` iterations of the
/// synthetic run `s` (plus the seed): every loop both laws get exactly the
/// same inputs, measured from the native law's last drive, and their outputs
/// are compared. Both must succeed on every call.
pub fn parity(
    native: &mut dyn MimoControlLaw,
    file: &mut dyn MimoControlLaw,
    s: &Synthetic,
    loops: usize,
) -> Parity {
    assert_eq!(
        native.needs(),
        file.needs(),
        "the two declare the same needs"
    );
    assert_eq!(
        native.shapes(),
        file.shapes(),
        "the two declare the same shapes"
    );
    let needs = native.needs().to_vec();
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let mut csd = cross(&s.plant, &last);
    let mut p = Parity {
        loops: 0,
        drive: 0.0,
        diagnostics: 0.0,
        drive_scale: 0.0,
    };
    for l in 0..=loops as u64 {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        let (a, b) = if l == 0 {
            (native.seed(&inputs), file.seed(&inputs))
        } else {
            (native.iterate(&inputs), file.iterate(&inputs))
        };
        let a = a.unwrap_or_else(|e| panic!("native loop {l}: {e}"));
        let b = b.unwrap_or_else(|e| panic!("law file loop {l}: {e}"));
        for line in 0..s.setup.lines {
            p.drive = p.drive.max(line_rel(&a.drive, &b.drive, line));
            let norm = a.drive.line(line).iter().map(|v| v.norm_sqr()).sum::<f64>();
            p.drive_scale = p.drive_scale.max(norm.sqrt());
        }
        p.diagnostics = p.diagnostics.max(diag_rel(&a, &b));
        p.loops += 1;
        last = a.drive;
        response = s.respond(&last, l + 1);
        csd = cross(&s.plant, &last);
    }
    p
}
