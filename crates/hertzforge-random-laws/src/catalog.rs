//! The law catalog: every control law an operator can choose from, the
//! built-in laws first, then the laws imported into the data root.
//! Single-output Random laws ([`LawCatalog::entries`]), multiple-output ones
//! ([`LawCatalog::mimo_entries`]) and classical shock laws
//! ([`LawCatalog::shock_classical_entries`]) are listed apart: a law file says
//! which world it implements when it is loaded, and each list holds only its
//! own.
//!
//! An imported law lives at `<data_root>/laws/<sha256>.wasm`, named by the
//! SHA-256 of its bytes, with its check record (if any) beside it at
//! `<sha256>.check.json`. The data root is the local one
//! (`hertzforge_capture::default_data_root()`, or a path the caller gives), never
//! a synced folder. A law reaches the catalog by [`LawCatalog::import`] from a
//! file path: files reach a service by path, they are never uploaded.

use crate::builtin::BuiltinLaw;
use crate::check::{is_sha256, CheckRecord};
use crate::mimo::builtin_mimo_manifest;
use hertzforge_law_host::{
    law_file_sha256, AnyLaw, DiagnosticSpec, HostConfig, LawHost, LawIdentity, Manifest,
    MimoManifest, NumberSpec, ParamKind, ParamSpec,
};
use hertzforge_mimo_laws::BuiltinMimo;
use hertzforge_shock_law_math::classical as fdc;
use hertzforge_shock_law_math::srs as srsr;
use hertzforge_shock_law_math::twr as twrc;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The folder of the data root that holds imported law files.
pub const LAWS_DIR: &str = "laws";

/// The largest law file the catalog reads, bytes. A control law is far
/// smaller; the cap keeps a wrong file from being read whole into memory.
pub const MAX_LAW_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Where an entry stands on the check that clears a law for hardware.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckStatus {
    /// A built-in law: part of the program, it needs no check record.
    BuiltIn,
    /// No check record for this file.
    Unchecked,
    /// The check record for this exact file (passed or not).
    Checked(CheckRecord),
    /// A check record is there but cannot be used: unreadable, malformed, or
    /// for another file.
    Unreadable(String),
}

/// Which test type a law is for: the world its file implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LawWorld {
    /// Single-output Random (`random-single`).
    Single,
    /// Multiple-output Random (`random-mimo`).
    Mimo,
    /// Classical shock (`shock-classical`).
    ShockClassical,
    /// SRS shock (`shock-srs`).
    ShockSrs,
    /// Single-output Sine (`sine-single`).
    Sine,
    /// Time waveform replication (`twr-single`). Listed with no entries until
    /// its built-in law and its law files are served.
    Twr,
}

impl LawWorld {
    /// The stable wire name: `single`, `mimo`, `shock_classical`,
    /// `shock_srs`, `sine` or `twr`.
    pub fn as_str(self) -> &'static str {
        match self {
            LawWorld::Single => "single",
            LawWorld::Mimo => "mimo",
            LawWorld::ShockClassical => "shock_classical",
            LawWorld::ShockSrs => "shock_srs",
            LawWorld::Sine => "sine",
            LawWorld::Twr => "twr",
        }
    }

    /// The test type, in words, for a refusal: "single-output Random",
    /// "multiple-output Random", "classical shock", "SRS shock", "Sine" or
    /// "TWR".
    pub fn test_type(self) -> &'static str {
        match self {
            LawWorld::Single => "single-output Random",
            LawWorld::Mimo => "multiple-output Random",
            LawWorld::ShockClassical => "classical shock",
            LawWorld::ShockSrs => "SRS shock",
            LawWorld::Sine => "Sine",
            LawWorld::Twr => "TWR",
        }
    }
}

/// The catalog id of the built-in classical shock law, `fd_correct`.
pub const CLASSICAL_BUILTIN_ID: &str = fdc::ID;

/// The built-in classical shock law's manifest, from its declaration in
/// `hertzforge-shock-law-math`: the same manifest its law file
/// (`laws/shock-fdc`) describes.
pub fn classical_builtin_manifest() -> Manifest {
    math_manifest(
        fdc::NAME,
        fdc::VERSION,
        fdc::SUMMARY,
        fdc::KNOBS,
        fdc::DIAGNOSTICS,
    )
}

/// The catalog id of the built-in SRS shock law, `srs_ratio`.
pub const SRS_BUILTIN_ID: &str = srsr::ID;

/// The built-in SRS shock law's manifest, from its declaration in
/// `hertzforge-shock-law-math`: the same manifest its law file
/// (`laws/shock-srs-ratio`) describes.
pub fn srs_builtin_manifest() -> Manifest {
    math_manifest(
        srsr::NAME,
        srsr::VERSION,
        srsr::SUMMARY,
        srsr::KNOBS,
        srsr::DIAGNOSTICS,
    )
}

/// The catalog id of the built-in TWR law, `twr_correct`.
pub const TWR_BUILTIN_ID: &str = twrc::ID;

/// The built-in TWR law's manifest, from its declaration in
/// `hertzforge-shock-law-math`: the same manifest its law file
/// (`laws/twr-correct`) describes.
pub fn twr_builtin_manifest() -> Manifest {
    math_manifest(
        twrc::NAME,
        twrc::VERSION,
        twrc::SUMMARY,
        twrc::KNOBS,
        twrc::DIAGNOSTICS,
    )
}

/// The catalog id of the built-in Sine law, `servo_blend`.
pub const SINE_BUILTIN_ID: &str = hertzforge_sine::law::SERVO_BLEND_ID;

/// The built-in Sine law's manifest, from its declaration in
/// `hertzforge-sine` (`law::servo_blend::KNOBS` and `DIAGNOSTICS`): the same
/// manifest its law file (`laws/sine-servo`) describes.
pub fn sine_builtin_manifest() -> Manifest {
    use hertzforge_law_host::{ChoiceSpec, DiagnosticSpec};
    use hertzforge_sine::law::servo_blend as sb;
    Manifest {
        name: sb::NAME.into(),
        version: sb::VERSION.into(),
        author: sb::AUTHOR.into(),
        summary: sb::SUMMARY.into(),
        params: sb::KNOBS
            .iter()
            .map(|k| ParamSpec {
                key: k.key.into(),
                label: k.label.into(),
                help: k.help.into(),
                kind: match k.kind {
                    sb::KnobKind::Number {
                        unit,
                        min,
                        max,
                        step,
                        default,
                    } => ParamKind::Number(NumberSpec {
                        unit: unit.into(),
                        min,
                        max,
                        step,
                        default,
                    }),
                    sb::KnobKind::Choice { options, default } => ParamKind::Choice(ChoiceSpec {
                        options: options.iter().map(|o| o.to_string()).collect(),
                        default: default.into(),
                    }),
                },
            })
            .collect(),
        diagnostics: sb::DIAGNOSTICS
            .iter()
            .map(|d| DiagnosticSpec {
                key: d.key.into(),
                label: d.label.into(),
                unit: d.unit.into(),
                help: d.help.into(),
            })
            .collect(),
    }
}

/// The built-in Sine law's live parameters: the keys an operator may change
/// while the test runs.
pub fn sine_builtin_live() -> Vec<String> {
    hertzforge_sine::law::servo_blend::KNOBS
        .iter()
        .filter(|k| k.live)
        .map(|k| k.key.to_string())
        .collect()
}

fn sine_builtin_entry() -> CatalogEntry {
    CatalogEntry {
        live: sine_builtin_live(),
        ..shock_builtin_entry(SINE_BUILTIN_ID, sine_builtin_manifest(), LawWorld::Sine)
    }
}

/// A built-in shock law's manifest from its declaration.
fn math_manifest(
    name: &str,
    version: &str,
    summary: &str,
    knobs: &[hertzforge_shock_law_math::NumberParam],
    diagnostics: &[hertzforge_shock_law_math::DiagnosticDecl],
) -> Manifest {
    Manifest {
        name: name.into(),
        version: version.into(),
        author: hertzforge_shock_law_math::AUTHOR.into(),
        summary: summary.into(),
        params: knobs
            .iter()
            .map(|k| ParamSpec {
                key: k.key.into(),
                label: k.label.into(),
                help: k.help.into(),
                kind: ParamKind::Number(NumberSpec {
                    unit: k.unit.into(),
                    min: k.min,
                    max: k.max,
                    step: k.step,
                    default: k.default,
                }),
            })
            .collect(),
        diagnostics: diagnostics
            .iter()
            .map(|d| DiagnosticSpec {
                key: d.key.into(),
                label: d.label.into(),
                unit: d.unit.into(),
                help: d.help.into(),
            })
            .collect(),
    }
}

fn classical_builtin_entry() -> CatalogEntry {
    shock_builtin_entry(
        CLASSICAL_BUILTIN_ID,
        classical_builtin_manifest(),
        LawWorld::ShockClassical,
    )
}

fn srs_builtin_entry() -> CatalogEntry {
    shock_builtin_entry(SRS_BUILTIN_ID, srs_builtin_manifest(), LawWorld::ShockSrs)
}

fn twr_builtin_entry() -> CatalogEntry {
    shock_builtin_entry(TWR_BUILTIN_ID, twr_builtin_manifest(), LawWorld::Twr)
}

fn shock_builtin_entry(id: &str, manifest: Manifest, world: LawWorld) -> CatalogEntry {
    CatalogEntry {
        id: id.into(),
        identity: LawIdentity {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            author: manifest.author.clone(),
            sha256: String::new(),
        },
        manifest,
        builtin: true,
        check: CheckStatus::BuiltIn,
        world,
        mimo: None,
        live: Vec::new(),
    }
}

/// One law the operator can choose.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogEntry {
    /// What a choice names the law by: the built-in id (`hinv`, `ratio`,
    /// `pinv_closed`, ...) or an imported file's SHA-256.
    pub id: String,
    /// Name, version and author from the manifest, and the file's SHA-256
    /// (empty for a built-in law).
    pub identity: LawIdentity,
    /// The law's manifest: its parameters and diagnostics.
    pub manifest: Manifest,
    /// Whether the law is built into the program.
    pub builtin: bool,
    /// Where the law stands on its check.
    pub check: CheckStatus,
    /// The test type the law is for.
    pub world: LawWorld,
    /// For a multiple-output law, its whole manifest: the shapes it supports,
    /// the optional inputs it reads and the layout of each diagnostic (its
    /// `law` is [`Self::manifest`]).
    pub mimo: Option<MimoManifest>,
    /// For a Sine law, the number parameters the operator may change while
    /// the test runs; empty for every other world.
    pub live: Vec<String>,
}

/// A file in the laws folder that is not in the catalog, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogProblem {
    /// The file.
    pub path: PathBuf,
    /// Why it was left out.
    pub reason: String,
}

/// What can go wrong opening the catalog or importing a law.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogError {
    /// A file or folder could not be read or written.
    Io {
        /// The path.
        path: PathBuf,
        /// What went wrong.
        reason: String,
    },
    /// The file is not a law this program can run.
    NotALaw {
        /// The file.
        path: PathBuf,
        /// Why, in the law host's words.
        reason: String,
    },
    /// The law host could not start.
    Host(String),
    /// A check record could not be written.
    Check(String),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatalogError::Io { path, reason } => write!(f, "{}: {reason}", path.display()),
            CatalogError::NotALaw { path, reason } => {
                write!(
                    f,
                    "{} is not a usable control law: {reason}",
                    path.display()
                )
            }
            CatalogError::Host(s) => write!(f, "the control-law host could not start: {s}"),
            CatalogError::Check(s) => write!(f, "the check record was not written: {s}"),
        }
    }
}

impl std::error::Error for CatalogError {}

/// An imported law file, loaded, of either world.
pub(crate) struct Imported {
    pub(crate) law: AnyLaw,
    pub(crate) check: CheckStatus,
}

/// The built-in laws and the laws imported into one data root.
pub struct LawCatalog {
    host: LawHost,
    laws_dir: PathBuf,
    imported: Vec<Imported>,
    problems: Vec<CatalogProblem>,
}

impl LawCatalog {
    /// The catalog for the default local data root
    /// (`hertzforge_capture::default_data_root()`).
    pub fn open_default() -> Result<Self, CatalogError> {
        Self::open(hertzforge_capture::default_data_root())
    }

    /// The catalog for `data_root`, with a law host on its default settings.
    pub fn open(data_root: impl Into<PathBuf>) -> Result<Self, CatalogError> {
        let host =
            LawHost::new(HostConfig::default()).map_err(|e| CatalogError::Host(e.to_string()))?;
        Self::with_host(host, data_root)
    }

    /// The catalog for `data_root`, loading law files with `host`. Reads every
    /// law file in `<data_root>/laws/`; a file that cannot be used is left out
    /// and listed in [`Self::problems`]. The folder is not created until a law
    /// is imported.
    pub fn with_host(host: LawHost, data_root: impl Into<PathBuf>) -> Result<Self, CatalogError> {
        let mut catalog = LawCatalog {
            host,
            laws_dir: data_root.into().join(LAWS_DIR),
            imported: Vec::new(),
            problems: Vec::new(),
        };
        catalog.rescan()?;
        Ok(catalog)
    }

    /// The folder imported laws live in.
    pub fn laws_dir(&self) -> &Path {
        &self.laws_dir
    }

    /// The host laws are loaded with.
    pub fn host(&self) -> &LawHost {
        &self.host
    }

    /// Files in the laws folder left out of the catalog at the last scan.
    pub fn problems(&self) -> &[CatalogProblem] {
        &self.problems
    }

    /// Read the laws folder again: law files and check records added, changed
    /// or removed by hand since the last scan.
    pub fn rescan(&mut self) -> Result<(), CatalogError> {
        self.imported.clear();
        self.problems.clear();
        let dir = match std::fs::read_dir(&self.laws_dir) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_err(&self.laws_dir, &e)),
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        for item in dir {
            let item = item.map_err(|e| io_err(&self.laws_dir, &e))?;
            let path = item.path();
            if path.extension().and_then(|x| x.to_str()) == Some("wasm") {
                paths.push(path);
            }
        }
        paths.sort();
        for path in paths {
            match self.load_stored(&path) {
                Ok(imported) => self.imported.push(imported),
                Err(reason) => self.problems.push(CatalogProblem { path, reason }),
            }
        }
        self.sort();
        Ok(())
    }

    fn sort(&mut self) {
        self.imported.sort_by(|a, b| {
            let (a, b) = (a.law.identity(), b.law.identity());
            (&a.name, &a.version, &a.sha256).cmp(&(&b.name, &b.version, &b.sha256))
        });
    }

    /// Load a stored law file: its name must be the SHA-256 of its bytes.
    fn load_stored(&self, path: &Path) -> Result<Imported, String> {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if !is_sha256(stem) {
            return Err("not named by the SHA-256 of its contents".into());
        }
        let bytes = read_capped(path).map_err(|e| e.to_string())?;
        let sha = law_file_sha256(&bytes);
        if sha != stem {
            return Err(format!(
                "its contents have the SHA-256 {sha}, not the one it is named by"
            ));
        }
        let law = self.host.load_any(&bytes).map_err(|e| e.to_string())?;
        Ok(Imported {
            check: check_status(&self.laws_dir, &sha),
            law,
        })
    }

    /// Every single-output law, the built-in laws first (the default first of
    /// all), then the imported single-output laws by name, version and hash.
    pub fn entries(&self) -> Vec<CatalogEntry> {
        BuiltinLaw::ALL
            .into_iter()
            .map(builtin_entry)
            .chain(
                self.imported
                    .iter()
                    .filter(|i| matches!(i.law, AnyLaw::Single(_)))
                    .map(imported_entry),
            )
            .collect()
    }

    /// Every multiple-output law, the built-in laws first (the default,
    /// `pinv_closed`, first of all), then the imported multiple-output laws by
    /// name, version and hash.
    pub fn mimo_entries(&self) -> Vec<CatalogEntry> {
        BuiltinMimo::ALL
            .into_iter()
            .map(builtin_mimo_entry)
            .chain(
                self.imported
                    .iter()
                    .filter(|i| matches!(i.law, AnyLaw::Mimo(_)))
                    .map(imported_entry),
            )
            .collect()
    }

    /// Every classical shock law, the built-in `fd_correct` (the default)
    /// first, then the imported `shock-classical` law files by name, version
    /// and hash.
    pub fn shock_classical_entries(&self) -> Vec<CatalogEntry> {
        std::iter::once(classical_builtin_entry())
            .chain(
                self.imported
                    .iter()
                    .filter(|i| matches!(i.law, AnyLaw::ShockClassical(_)))
                    .map(imported_entry),
            )
            .collect()
    }

    /// Every SRS shock law, the built-in `srs_ratio` (the default) first,
    /// then the imported `shock-srs` law files by name, version and hash.
    pub fn shock_srs_entries(&self) -> Vec<CatalogEntry> {
        std::iter::once(srs_builtin_entry())
            .chain(
                self.imported
                    .iter()
                    .filter(|i| matches!(i.law, AnyLaw::ShockSrs(_)))
                    .map(imported_entry),
            )
            .collect()
    }

    /// Every Sine law, the built-in `servo_blend` (the default) first, then
    /// the imported `sine-single` law files by name, version and hash.
    pub fn sine_entries(&self) -> Vec<CatalogEntry> {
        std::iter::once(sine_builtin_entry())
            .chain(
                self.imported
                    .iter()
                    .filter(|i| matches!(i.law, AnyLaw::Sine(_)))
                    .map(imported_entry),
            )
            .collect()
    }

    /// The imported Sine law file with `id` (its SHA-256).
    pub fn sine_law(&self, id: &str) -> Option<&hertzforge_law_host::LoadedSineLaw> {
        match self.imported_law(id).map(|i| &i.law) {
            Some(AnyLaw::Sine(l)) => Some(l),
            _ => None,
        }
    }

    /// Every TWR law, the built-in `twr_correct` (the default) first, then the
    /// imported `twr-single` law files by name, version and hash.
    pub fn twr_entries(&self) -> Vec<CatalogEntry> {
        std::iter::once(twr_builtin_entry())
            .chain(
                self.imported
                    .iter()
                    .filter(|i| matches!(i.law, AnyLaw::Twr(_)))
                    .map(imported_entry),
            )
            .collect()
    }

    /// The imported TWR law file with `id` (its SHA-256).
    pub fn twr_law(&self, id: &str) -> Option<&hertzforge_law_host::LoadedTwrLaw> {
        match self.imported_law(id).map(|i| &i.law) {
            Some(AnyLaw::Twr(l)) => Some(l),
            _ => None,
        }
    }

    /// Every law of `world`.
    pub fn entries_for(&self, world: LawWorld) -> Vec<CatalogEntry> {
        match world {
            LawWorld::Single => self.entries(),
            LawWorld::Mimo => self.mimo_entries(),
            LawWorld::ShockClassical => self.shock_classical_entries(),
            LawWorld::ShockSrs => self.shock_srs_entries(),
            LawWorld::Sine => self.sine_entries(),
            LawWorld::Twr => self.twr_entries(),
        }
    }

    /// The imported classical shock law file with `id` (its SHA-256).
    pub fn classical_law(&self, id: &str) -> Option<&hertzforge_law_host::LoadedClassicalLaw> {
        match self.imported_law(id).map(|i| &i.law) {
            Some(AnyLaw::ShockClassical(l)) => Some(l),
            _ => None,
        }
    }

    /// The imported SRS shock law file with `id` (its SHA-256).
    pub fn srs_law(&self, id: &str) -> Option<&hertzforge_law_host::LoadedSrsLaw> {
        match self.imported_law(id).map(|i| &i.law) {
            Some(AnyLaw::ShockSrs(l)) => Some(l),
            _ => None,
        }
    }

    /// The law with `id`, of any world.
    pub fn entry(&self, id: &str) -> Option<CatalogEntry> {
        if let Some(b) = BuiltinLaw::from_id(id) {
            return Some(builtin_entry(b));
        }
        if let Some(b) = BuiltinMimo::from_id(id) {
            return Some(builtin_mimo_entry(b));
        }
        if id == CLASSICAL_BUILTIN_ID {
            return Some(classical_builtin_entry());
        }
        if id == SRS_BUILTIN_ID {
            return Some(srs_builtin_entry());
        }
        if id == SINE_BUILTIN_ID {
            return Some(sine_builtin_entry());
        }
        if id == TWR_BUILTIN_ID {
            return Some(twr_builtin_entry());
        }
        self.imported_law(id).map(imported_entry)
    }

    pub(crate) fn imported_law(&self, id: &str) -> Option<&Imported> {
        self.imported.iter().find(|i| i.law.identity().sha256 == id)
    }

    /// Import the law file at `path`: load it with the host (which compiles
    /// it, finds which world it implements, single-output or multiple-output
    /// Random, and checks its manifest), then copy it into the laws folder
    /// under its SHA-256. Importing a file already in the catalog returns its
    /// entry and copies nothing. The file at `path` is not changed.
    pub fn import(&mut self, path: &Path) -> Result<CatalogEntry, CatalogError> {
        let bytes = read_capped(path)?;
        let law = self
            .host
            .load_any(&bytes)
            .map_err(|e| CatalogError::NotALaw {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
        let sha = law.identity().sha256.clone();
        if let Some(existing) = self.imported_law(&sha) {
            return Ok(imported_entry(existing));
        }
        let stored = self.laws_dir.join(format!("{sha}.wasm"));
        write_whole(&self.laws_dir, &stored, &bytes).map_err(|reason| CatalogError::Io {
            path: stored.clone(),
            reason,
        })?;
        self.problems.retain(|p| p.path != stored);
        self.imported.push(Imported {
            check: check_status(&self.laws_dir, &sha),
            law,
        });
        self.sort();
        let entry =
            self.imported_law(&sha)
                .map(imported_entry)
                .ok_or_else(|| CatalogError::Io {
                    path: stored,
                    reason: "the imported law is not in the catalog".into(),
                })?;
        Ok(entry)
    }

    /// Where an imported law stands on the further check `check` (for example
    /// `mixed-floor`): read from its record now, so a record written or
    /// removed since the last scan is seen. A record for another file, or a
    /// malformed one, is [`CheckStatus::Unreadable`]. The law's own check
    /// ([`CatalogEntry::check`]) is not changed by a further check.
    pub fn named_check(&self, law_sha256: &str, check: &str) -> CheckStatus {
        match CheckRecord::read_named(&self.laws_dir, law_sha256, check) {
            Ok(Some(r)) => CheckStatus::Checked(r),
            Ok(None) => CheckStatus::Unchecked,
            Err(why) => CheckStatus::Unreadable(why),
        }
    }

    /// Write `record` as the record of the further check `check` beside its
    /// law file. The law's entry, and its own check record, are unchanged.
    pub fn record_named_check(
        &mut self,
        check: &str,
        record: &CheckRecord,
    ) -> Result<(), CatalogError> {
        record
            .write_named(&self.laws_dir, check)
            .map_err(CatalogError::Check)?;
        Ok(())
    }

    /// Write `record` beside its law file and update that law's entry.
    pub fn record_check(&mut self, record: &CheckRecord) -> Result<(), CatalogError> {
        record.write(&self.laws_dir).map_err(CatalogError::Check)?;
        let laws_dir = self.laws_dir.clone();
        if let Some(i) = self
            .imported
            .iter_mut()
            .find(|i| i.law.identity().sha256 == record.law_sha256)
        {
            i.check = check_status(&laws_dir, &record.law_sha256);
        }
        Ok(())
    }
}

fn builtin_entry(b: BuiltinLaw) -> CatalogEntry {
    CatalogEntry {
        id: b.id().to_string(),
        identity: b.identity(),
        manifest: b.manifest(),
        builtin: true,
        check: CheckStatus::BuiltIn,
        world: LawWorld::Single,
        mimo: None,
        live: Vec::new(),
    }
}

fn builtin_mimo_entry(b: BuiltinMimo) -> CatalogEntry {
    let m = builtin_mimo_manifest(b);
    CatalogEntry {
        id: b.id().to_string(),
        identity: LawIdentity {
            name: m.law.name.clone(),
            version: m.law.version.clone(),
            author: m.law.author.clone(),
            sha256: String::new(),
        },
        manifest: m.law.clone(),
        builtin: true,
        check: CheckStatus::BuiltIn,
        world: LawWorld::Mimo,
        mimo: Some(m),
        live: Vec::new(),
    }
}

fn imported_entry(i: &Imported) -> CatalogEntry {
    let (manifest, world, mimo) = match &i.law {
        AnyLaw::Single(l) => (l.manifest().clone(), LawWorld::Single, None),
        AnyLaw::Mimo(l) => (
            l.manifest().law.clone(),
            LawWorld::Mimo,
            Some(l.manifest().clone()),
        ),
        AnyLaw::ShockClassical(l) => (l.manifest().clone(), LawWorld::ShockClassical, None),
        AnyLaw::ShockSrs(l) => (l.manifest().clone(), LawWorld::ShockSrs, None),
        AnyLaw::Sine(l) => (l.manifest().clone(), LawWorld::Sine, None),
        AnyLaw::Twr(l) => (l.manifest().clone(), LawWorld::Twr, None),
    };
    CatalogEntry {
        id: i.law.identity().sha256.clone(),
        identity: i.law.identity().clone(),
        manifest,
        builtin: false,
        check: i.check.clone(),
        world,
        mimo,
        live: match &i.law {
            AnyLaw::Sine(l) => l.live().to_vec(),
            _ => Vec::new(),
        },
    }
}

fn check_status(laws_dir: &Path, sha: &str) -> CheckStatus {
    match CheckRecord::read(laws_dir, sha) {
        Ok(Some(r)) => CheckStatus::Checked(r),
        Ok(None) => CheckStatus::Unchecked,
        Err(why) => CheckStatus::Unreadable(why),
    }
}

fn io_err(path: &Path, e: &std::io::Error) -> CatalogError {
    CatalogError::Io {
        path: path.to_path_buf(),
        reason: e.to_string(),
    }
}

/// The bytes of the regular file at `path`, refusing one over
/// [`MAX_LAW_FILE_BYTES`].
fn read_capped(path: &Path) -> Result<Vec<u8>, CatalogError> {
    let meta = std::fs::metadata(path).map_err(|e| io_err(path, &e))?;
    if !meta.is_file() {
        return Err(CatalogError::Io {
            path: path.to_path_buf(),
            reason: "not a file".into(),
        });
    }
    let file = std::fs::File::open(path).map_err(|e| io_err(path, &e))?;
    let mut bytes = Vec::new();
    file.take(MAX_LAW_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(path, &e))?;
    if bytes.len() as u64 > MAX_LAW_FILE_BYTES {
        return Err(CatalogError::Io {
            path: path.to_path_buf(),
            reason: format!("larger than the {MAX_LAW_FILE_BYTES} bytes a law file may be"),
        });
    }
    Ok(bytes)
}

/// Write `bytes` to `path` in `dir` (created if missing) whole or not at all:
/// into a temporary file in the same folder, then renamed over `path`.
pub(crate) fn write_whole(dir: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let tmp = dir.join(format!(".{name}.{}.part", std::process::id()));
    let written = std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("{}: {e}", path.display()));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}
