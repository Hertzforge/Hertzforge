//! The law catalog: every Random control law an operator can choose from, the
//! built-in laws first, then the laws imported into the data root.
//!
//! An imported law lives at `<data_root>/laws/<sha256>.wasm`, named by the
//! SHA-256 of its bytes, with its check record (if any) beside it at
//! `<sha256>.check.json`. The data root is the local one
//! (`hertzforge_capture::default_data_root()`, or a path the caller gives), never
//! a synced folder. A law reaches the catalog by [`LawCatalog::import`] from a
//! file path: files reach a service by path, they are never uploaded.

use crate::builtin::BuiltinLaw;
use crate::check::{is_sha256, CheckRecord};
use hertzforge_law_host::{law_file_sha256, HostConfig, LawHost, LawIdentity, LoadedLaw, Manifest};
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

/// One law the operator can choose.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogEntry {
    /// What a choice names the law by: the built-in id (`hinv`, `ratio`) or an
    /// imported file's SHA-256.
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

/// An imported law file, loaded.
pub(crate) struct Imported {
    pub(crate) law: LoadedLaw,
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
        let law = self.host.load(&bytes).map_err(|e| e.to_string())?;
        Ok(Imported {
            check: check_status(&self.laws_dir, &sha),
            law,
        })
    }

    /// Every law, the built-in laws first (the default first of all), then the
    /// imported laws by name, version and hash.
    pub fn entries(&self) -> Vec<CatalogEntry> {
        BuiltinLaw::ALL
            .into_iter()
            .map(builtin_entry)
            .chain(self.imported.iter().map(imported_entry))
            .collect()
    }

    /// The law with `id`.
    pub fn entry(&self, id: &str) -> Option<CatalogEntry> {
        if let Some(b) = BuiltinLaw::from_id(id) {
            return Some(builtin_entry(b));
        }
        self.imported_law(id).map(imported_entry)
    }

    pub(crate) fn imported_law(&self, id: &str) -> Option<&Imported> {
        self.imported.iter().find(|i| i.law.identity().sha256 == id)
    }

    /// Import the law file at `path`: load it with the host (which compiles it
    /// and checks that it is a single-output Random law with a sound manifest),
    /// then copy it into the laws folder under its SHA-256. Importing a file
    /// already in the catalog returns its entry and copies nothing. The file at
    /// `path` is not changed.
    pub fn import(&mut self, path: &Path) -> Result<CatalogEntry, CatalogError> {
        let bytes = read_capped(path)?;
        let law = self.host.load(&bytes).map_err(|e| CatalogError::NotALaw {
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
    }
}

fn imported_entry(i: &Imported) -> CatalogEntry {
    CatalogEntry {
        id: i.law.identity().sha256.clone(),
        identity: i.law.identity().clone(),
        manifest: i.law.manifest().clone(),
        builtin: false,
        check: i.check.clone(),
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
