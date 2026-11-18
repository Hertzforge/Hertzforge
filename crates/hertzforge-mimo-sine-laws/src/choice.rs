//! A catalog id to the factory a run builds its law with: a built-in law by
//! name ([`resolve_builtin`]), or a `sine-mimo` law file in the catalog by its
//! SHA-256 ([`resolve`]); and a law file by path ([`load_law_file`]).

use crate::native::MathLawFactory;
use crate::wasm::WasmSineMimoLawFactory;
use hertzforge_law_host::{LawHost, LoadedSineMimoLaw, Param};
use hertzforge_law_sdk::sine_mimo::builtin::{self as decl, LawDecl};
use hertzforge_mimo_sine::MimoSineLawFactory;
use hertzforge_random_laws::{LawCatalog, LawWorld};
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Why a law could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No built-in law and no law file in the catalog has this id.
    Unknown(String),
    /// The id is a law file of another test type.
    WrongWorld {
        /// The id asked for.
        id: String,
        /// The test type the law file is for.
        law_is_for: &'static str,
    },
    /// The law file could not be read or loaded.
    File(String),
    /// A parameter the law does not declare, or a value it cannot take.
    BadParams(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Unknown(id) => write!(f, "no MIMO Sine law has the id `{id}`"),
            ResolveError::WrongWorld { id, law_is_for } => write!(
                f,
                "the law `{id}` is a {law_is_for} law, not a MIMO Sine law"
            ),
            ResolveError::File(why) => write!(f, "the MIMO Sine law file: {why}"),
            ResolveError::BadParams(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// The declaration of the built-in law `id`.
pub fn builtin_decl(id: &str) -> Option<&'static LawDecl> {
    decl::decl(id)
}

/// A host parameter value as the SDK's.
pub fn sdk_value(v: &hertzforge_law_host::ParamValue) -> hertzforge_law_sdk::ParamValue {
    use hertzforge_law_host::ParamValue as H;
    use hertzforge_law_sdk::ParamValue as S;
    match v {
        H::Number(x) => S::Number(*x),
        H::Integer(x) => S::Integer(*x),
        H::Toggle(x) => S::Toggle(*x),
        H::Choice(x) => S::Choice(x.clone()),
        H::PerChannel(x) => S::PerChannel(x.clone()),
        H::Curve(x) => S::Curve(
            x.iter()
                .map(|b| hertzforge_law_sdk::Breakpoint {
                    frequency_hz: b.frequency_hz,
                    value: b.value,
                })
                .collect(),
        ),
    }
}

/// The factory for the built-in law `id` at its declared defaults.
pub fn resolve_builtin(id: &str) -> Result<Arc<dyn MimoSineLawFactory>, ResolveError> {
    resolve_builtin_with(id, &[])
}

/// The factory for the built-in law `id` with `params` set over its declared
/// defaults. The values are checked against the declaration (key, kind and
/// range, as the law host checks a law file's) and read by the SDK's built-in
/// constructor, the same reader the law's file uses: one set of keys, ranges
/// and defaults.
pub fn resolve_builtin_with(
    id: &str,
    params: &[Param],
) -> Result<Arc<dyn MimoSineLawFactory>, ResolveError> {
    let Some(d) = builtin_decl(id) else {
        return Err(ResolveError::Unknown(id.into()));
    };
    let manifest = crate::facts::host_manifest(d);
    let checked = hertzforge_law_host::validate::resolve_params(&manifest, params, 1)
        .map_err(|e| ResolveError::BadParams(format!("the law `{id}`: {e}")))?;
    let all = hertzforge_law_sdk::Params(
        checked
            .iter()
            .map(|p| hertzforge_law_sdk::Param {
                key: p.key.clone(),
                value: sdk_value(&p.value),
            })
            .collect(),
    );
    MathLawFactory::new(d, all)
        .map(|f| Arc::new(f) as Arc<dyn MimoSineLawFactory>)
        .map_err(|e| ResolveError::BadParams(format!("the law `{id}`: {e}")))
}

/// The factory a run builds its law with, for the law with catalog id `id`:
/// a built-in by name ([`resolve_builtin_with`]), or a `sine-mimo` law file
/// in `catalog` by its SHA-256, built with `params`, with `budget` as every
/// update's deadline and the default build budget for its constructor.
pub fn resolve(
    catalog: &LawCatalog,
    id: &str,
    params: Vec<Param>,
    budget: Duration,
) -> Result<Arc<dyn MimoSineLawFactory>, ResolveError> {
    if builtin_decl(id).is_some() {
        return resolve_builtin_with(id, &params);
    }
    if let Some(law) = catalog.sine_mimo_law(id) {
        return Ok(Arc::new(WasmSineMimoLawFactory::new(
            law.clone(),
            params,
            budget,
        )));
    }
    match catalog.entry(id) {
        Some(e) if e.world != LawWorld::SineMimo => Err(ResolveError::WrongWorld {
            id: id.into(),
            law_is_for: e.world.test_type(),
        }),
        _ => Err(ResolveError::Unknown(id.into())),
    }
}

/// [`resolve`] with the law file's constructor given `build_budget`.
pub fn resolve_for_run(
    catalog: &LawCatalog,
    id: &str,
    params: Vec<Param>,
    budget: Duration,
    build_budget: Duration,
) -> Result<Arc<dyn MimoSineLawFactory>, ResolveError> {
    if builtin_decl(id).is_none() {
        if let Some(law) = catalog.sine_mimo_law(id) {
            return Ok(Arc::new(
                WasmSineMimoLawFactory::new(law.clone(), params, budget)
                    .with_build_budget(build_budget),
            ));
        }
    }
    resolve(catalog, id, params, budget)
}

/// Load the MIMO Sine law file at `path` with `host`: how a contributor runs
/// a law straight from the build, before importing it into a catalog.
pub fn load_law_file(host: &LawHost, path: &Path) -> Result<LoadedSineMimoLaw, ResolveError> {
    let bytes =
        std::fs::read(path).map_err(|e| ResolveError::File(format!("{}: {e}", path.display())))?;
    host.load_sine_mimo(&bytes)
        .map_err(|e| ResolveError::File(format!("{}: {e}", path.display())))
}
