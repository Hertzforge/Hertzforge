//! A MIMO Mixed Mode configuration on the wire (schema/PROTOCOL.md, "MIMO Mixed
//! Mode"): the controller's own configuration (`hertzforge_mimo_mixed::MimoMixedConfig`,
//! its serde form) with the members only the wire has, and the checks configure
//! runs on it against the channel table and the law catalog.
//!
//! The wire's own members, each named as the sibling module that has the same
//! thing names it:
//!
//! - `estimation.fs_hz` and `estimation.n_pts`: the grid, as MIMO Random's setup
//!   names it (the controller takes its grid from the reference);
//! - `reference`: MIMO Random's (`{"kind": "inline", "freqs_hz", "n", "re", "im"}`
//!   or `{"kind": "file", "path", "sha256"}`), put on the grid here;
//! - `title`, `operator`, `article`, `notes`, `specification` and
//!   `record.streams`: the record's words, as every module names them;
//! - `simulator.structure`: the simulator's sine structure, as MIMO Sine's.
//!
//! The laws resolve here too: the floor's through MIMO Random's catalog path
//! (a built-in by `BuiltinMimo`, a law file by `LawCatalog::resolve_mimo`), the
//! tones' through MIMO Sine's (`crate::mimo_sine::laws::resolve`), both with their
//! parameters; on a hardware station each role's law must clear
//! `hertzforge_mimo_mixed_laws::may_arm_in_role`.

use crate::protocol::{ErrorCode, MethodError};
use crate::table::{Item, Report};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_law_host::ParamSpec;
use hertzforge_mimo_mixed::service::{LawRole, MimoMixedLaws};
use hertzforge_mimo_mixed::{MimoMixedConfig, MimoPartBody};
use hertzforge_random_laws::{builtin_mimo_manifest, BuiltinMimo, LawCatalog, LawChoice, LawWorld};
use hertzforge_reference::{HermitianStack, Method, Provenance, Reference, Tolerances, C64};
use serde_json::{json, Map, Value};
use std::time::Duration;

/// How long a law file may take to build its law.
pub const BUILD_BUDGET: Duration = Duration::from_secs(120);

/// The reference as the wire carries it (MIMO Random's two forms).
#[derive(Debug, Clone, PartialEq)]
pub enum WireReference {
    Inline {
        freqs_hz: Vec<f64>,
        n: usize,
        re: Vec<f64>,
        im: Vec<f64>,
    },
    File {
        path: String,
        sha256: String,
    },
}

/// A configuration as parsed: the wire's members, and the rest for the controller.
#[derive(Debug, Clone, PartialEq)]
pub struct WireMimoMixed {
    /// The configuration as accepted.
    pub raw: Value,
    pub title: String,
    pub operator: String,
    pub article: String,
    pub notes: String,
    pub specification: Option<(String, String)>,
    /// `record.streams`: whether the run keeps its time histories.
    pub streams: bool,
    /// `simulator.structure`.
    pub structure: Option<String>,
    pub fs_hz: f64,
    pub n_pts: usize,
    pub reference: Option<WireReference>,
    /// The controller's members, the reference left out.
    pub core: Map<String, Value>,
}

impl WireMimoMixed {
    /// The grid's line spacing, Hz.
    pub fn df_hz(&self) -> f64 {
        self.fs_hz / self.n_pts.max(1) as f64
    }

    /// The grid's lines (0 Hz to the Nyquist line).
    pub fn lines(&self) -> usize {
        self.n_pts / 2 + 1
    }
}

fn text(o: &Map<String, Value>, key: &str) -> String {
    o.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Parse `config`: an object; the wire's members taken out of it.
pub fn parse(config: &Value) -> Result<WireMimoMixed, MethodError> {
    let Some(o) = config.as_object() else {
        return Err(MethodError::bad_params(
            "config",
            "A MIMO Mixed Mode configuration is an object.",
        ));
    };
    let mut core = o.clone();
    for k in [
        "title",
        "operator",
        "article",
        "notes",
        "specification",
        "record",
        "simulator",
        "reference",
    ] {
        core.remove(k);
    }
    let est = o.get("estimation").and_then(Value::as_object);
    let fs_hz = est
        .and_then(|e| e.get("fs_hz"))
        .and_then(Value::as_f64)
        .unwrap_or(f64::NAN);
    let n_pts = est
        .and_then(|e| e.get("n_pts"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    if let Some(Value::Object(e)) = core.get_mut("estimation") {
        e.remove("fs_hz");
        e.remove("n_pts");
    }
    // The schedule's mode as Random's wire names it (`auto`, `manual`).
    if let Some(Value::Object(s)) = core.get_mut("schedule") {
        let mode = match s.get("mode").and_then(Value::as_str) {
            Some("auto") => Some("Auto"),
            Some("manual") => Some("Manual"),
            _ => None,
        };
        if let Some(m) = mode {
            s.insert("mode".into(), Value::from(m));
        }
    }
    let reference = match o.get("reference") {
        Some(Value::Object(r)) => match r.get("kind").and_then(Value::as_str) {
            Some("inline") => {
                let nums = |k: &str| -> Vec<f64> {
                    r.get(k)
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect())
                        .unwrap_or_default()
                };
                Some(WireReference::Inline {
                    freqs_hz: nums("freqs_hz"),
                    n: r.get("n").and_then(Value::as_u64).unwrap_or(0) as usize,
                    re: nums("re"),
                    im: nums("im"),
                })
            }
            Some("file") => Some(WireReference::File {
                path: text(r, "path"),
                sha256: text(r, "sha256"),
            }),
            _ => None,
        },
        _ => None,
    };
    let specification = o.get("specification").and_then(|s| {
        let s = s.as_object()?;
        Some((text(s, "document"), text(s, "clause")))
    });
    Ok(WireMimoMixed {
        raw: config.clone(),
        title: o
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("MIMO Mixed Mode")
            .to_owned(),
        operator: text(o, "operator"),
        article: text(o, "article"),
        notes: text(o, "notes"),
        specification,
        streams: o
            .get("record")
            .and_then(|r| r.get("streams"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        structure: o
            .get("simulator")
            .and_then(|s| s.get("structure"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        fs_hz,
        n_pts,
        reference,
        core,
    })
}

/// One role's law as the checks saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleLaw {
    pub role: LawRole,
    pub id: String,
    pub name: String,
    pub version: String,
    /// Empty for a built-in law.
    pub sha256: String,
    pub builtin: bool,
    pub params: Vec<ParamSpec>,
    /// The tone law's test types (`sweep`, `dwell`, `stepped`); empty for the floor's.
    pub test_types: Vec<&'static str>,
    /// Whether it may drive hardware in its role now.
    pub hardware: bool,
    /// Why it may not, when it may not.
    pub not_cleared: Option<String>,
}

impl RoleLaw {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "version": self.version,
            "sha256": if self.sha256.is_empty() { Value::Null } else { Value::from(self.sha256.as_str()) },
            "builtin": self.builtin,
            "world": hertzforge_mimo_mixed_laws::role_world(self.role).as_str(),
            "hardware": self.hardware,
            "not_cleared": self.not_cleared,
        })
    }
}

/// Both roles' laws.
#[derive(Debug, Clone, PartialEq)]
pub struct LawFacts {
    pub floor: RoleLaw,
    pub tones: RoleLaw,
}

pub fn role_name(role: LawRole) -> &'static str {
    match role {
        LawRole::Floor => "floor",
        LawRole::Tones => "tones",
    }
}

fn clearance(cat: &LawCatalog, id: &str, role: LawRole, builtin: bool) -> Result<(), String> {
    if builtin {
        return Ok(());
    }
    let world = hertzforge_mimo_mixed_laws::role_world(role);
    let entry = cat
        .entries_for(world)
        .into_iter()
        .find(|e| e.id == id)
        .or_else(|| cat.entry(id));
    match entry {
        Some(e) => hertzforge_mimo_mixed_laws::may_arm_in_role(cat, &e, role)
            .map_err(|why| why.to_string()),
        None => Err("it is not in the law catalog".into()),
    }
}

/// The floor's law `id`, from the MIMO Random catalog path: a built-in MIMO
/// Random law or a `mimo` law file. `Err` is the item's reason and words.
pub fn floor_law(cat: &LawCatalog, id: &str) -> Result<RoleLaw, (&'static str, String)> {
    if let Some(e) = cat
        .entries_for(LawWorld::Mimo)
        .into_iter()
        .find(|e| e.id == id)
    {
        let clear = clearance(cat, id, LawRole::Floor, e.builtin);
        return Ok(RoleLaw {
            role: LawRole::Floor,
            id: e.id.clone(),
            name: e.identity.name.clone(),
            version: e.identity.version.clone(),
            sha256: e.identity.sha256.clone(),
            builtin: e.builtin,
            params: e.manifest.params.clone(),
            test_types: Vec::new(),
            hardware: clear.is_ok(),
            not_cleared: clear.err(),
        });
    }
    if let Some(b) = BuiltinMimo::from_id(id) {
        let m = builtin_mimo_manifest(b);
        return Ok(RoleLaw {
            role: LawRole::Floor,
            id: id.to_owned(),
            name: m.law.name.clone(),
            version: m.law.version.clone(),
            sha256: String::new(),
            builtin: true,
            params: m.law.params.clone(),
            test_types: Vec::new(),
            hardware: true,
            not_cleared: None,
        });
    }
    match cat.entry(id) {
        Some(e) => Err((
            "law_wrong_world",
            format!(
                "\"{}\" is a {} law; the floor takes a MIMO Random (mimo) law.",
                e.identity.name,
                e.world.as_str()
            ),
        )),
        None => Err((
            "law_unknown",
            format!("No MIMO Random law \"{id}\" is built in or in the catalog; import it first."),
        )),
    }
}

/// The tones' law `id`, from the MIMO Sine catalog path: a built-in MIMO Sine
/// law or a `sine_mimo` law file.
pub fn tone_law(cat: &LawCatalog, id: &str) -> Result<RoleLaw, (&'static str, String)> {
    let Some(f) = crate::mimo_sine::laws::facts(cat, id) else {
        return Err((
            "law_unknown",
            format!("No MIMO Sine law \"{id}\" is built in or in the catalog; import it first."),
        ));
    };
    if let Err(world) = f.world {
        return Err((
            "law_wrong_world",
            format!(
                "\"{}\" is a {world} law; the tones take a MIMO Sine (sine_mimo) law.",
                f.name
            ),
        ));
    }
    let clear = clearance(cat, id, LawRole::Tones, f.builtin);
    Ok(RoleLaw {
        role: LawRole::Tones,
        id: f.id,
        name: f.name,
        version: f.version,
        sha256: f.sha256,
        builtin: f.builtin,
        params: f.params,
        test_types: f.test_types.iter().map(|t| t.as_str()).collect(),
        hardware: clear.is_ok(),
        not_cleared: clear.err(),
    })
}

/// What configure derived, for its answer and the record.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub fs_hz: f64,
    pub n_pts: usize,
    pub df_hz: f64,
    pub lines: usize,
    /// The floor's band, lines `[lo, hi)`.
    pub band: (usize, usize),
    /// Each control's reference rms at full level.
    pub reference_rms: Vec<f64>,
    pub law: LawFacts,
    /// The reference's source, in words.
    pub source: String,
}

impl Derived {
    pub fn to_json(&self) -> Value {
        json!({
            "grid": {"fs_hz": self.fs_hz, "n_pts": self.n_pts, "df_hz": self.df_hz,
                     "lines": self.lines},
            "band_hz": [self.band.0 as f64 * self.df_hz, (self.band.1 - 1) as f64 * self.df_hz],
            "reference_rms": self.reference_rms.iter().map(|x| crate::protocol::num(sig(*x, 6))).collect::<Vec<_>>(),
            "reference_source": self.source,
            "law": {"floor": self.law.floor.to_json(), "tones": self.law.tones.to_json()},
        })
    }
}

pub fn round(x: f64, digits: i32) -> f64 {
    let p = 10f64.powi(digits);
    (x * p).round() / p
}

pub fn sig(x: f64, digits: i32) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let mag = x.abs().log10().floor() as i32;
    round(x, digits - 1 - mag)
}

/// The inline reference on the grid (MIMO Random's interpolation): every grid
/// line inside the reference's first and last frequency, each packed element
/// linear between the two reference lines around it; zero outside.
fn inline_on_grid(
    w: &WireMimoMixed,
    freqs: &[f64],
    n: usize,
    re: &[f64],
    im: &[f64],
) -> Result<(HermitianStack, (usize, usize)), String> {
    let (lines, df) = (w.lines(), w.df_hz());
    let per = HermitianStack::packed_len(n);
    if n == 0 || freqs.len() < 2 || re.len() != freqs.len() * per || im.len() != re.len() {
        return Err("the inline reference's shape does not match its line count".into());
    }
    let (f0, f1) = (freqs[0], freqs[freqs.len() - 1]);
    let eps = 1e-9 * df;
    let lo = ((f0 - eps) / df).ceil().max(1.0) as usize;
    let hi = (((f1 + eps) / df).floor() as usize + 1).min(lines);
    if hi <= lo + 1 {
        return Err(format!(
            "the reference spans {f0} to {f1} Hz, fewer than two lines of the {df} Hz grid"
        ));
    }
    let mut sdm = HermitianStack::zeros(lines, n);
    let mut k = 0usize;
    for line in lo..hi {
        let f = line as f64 * df;
        while k + 2 < freqs.len() && freqs[k + 1] < f {
            k += 1;
        }
        let (fa, fb) = (freqs[k], freqs[k + 1]);
        let a_w = if fb > fa {
            ((f - fa) / (fb - fa)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        for r in 0..n {
            for c in r..n {
                let p = HermitianStack::packed_index(n, r, c);
                let a = C64::new(re[k * per + p], im[k * per + p]);
                let b = C64::new(re[(k + 1) * per + p], im[(k + 1) * per + p]);
                sdm.set(line, r, c, a * (1.0 - a_w) + b * a_w);
            }
        }
    }
    Ok((sdm, (lo, hi)))
}

/// Each control's (alarm, abort) bands about the reference, dB `(below, above)`,
/// from the floor part.
pub fn floor_bands(cfg: &MimoMixedConfig) -> Vec<((f64, f64), (f64, f64))> {
    let n = cfg.n();
    let Some(f) = cfg.floor() else {
        return vec![((3.0, 3.0), (6.0, 6.0)); n];
    };
    (0..n)
        .map(|i| (per_control(&f.alarm_db, i), per_control(&f.abort_db, i)))
        .collect()
}

/// The value of a per-control statement for control `i`.
pub fn per_control<T: Clone>(p: &hertzforge_mimo_mixed::config::PerControl<T>, i: usize) -> T {
    use hertzforge_mimo_mixed::config::PerControl;
    match p {
        PerControl::All(x) => x.clone(),
        PerControl::Each(v) => v.get(i).or(v.last()).cloned().expect("a stated value"),
    }
}

/// A control's alarm and abort bands, `((minus, plus), (minus, plus))` dB.
pub type BandPair = ((f64, f64), (f64, f64));

/// The tolerance lines about `sdm` on `band`, from `bands`.
pub fn tolerances_of(sdm: &HermitianStack, band: (usize, usize), bands: &[BandPair]) -> Tolerances {
    let n = sdm.n();
    let mut t = Tolerances::none(sdm.lines(), n);
    for (i, ((al, ah), (bl, bh))) in bands.iter().enumerate().take(n) {
        for line in band.0..band.1 {
            let p = sdm.get(line, i, i).re;
            let k = line * n + i;
            t.alarm_upper[k] = p * 10f64.powf(ah / 10.0);
            t.alarm_lower[k] = p * 10f64.powf(-al / 10.0);
            t.abort_upper[k] = p * 10f64.powf(bh / 10.0);
            t.abort_lower[k] = p * 10f64.powf(-bl / 10.0);
        }
    }
    t
}

/// The reference on the grid, and its source in words.
fn reference_on_grid(
    w: &WireMimoMixed,
    labels: Vec<String>,
    unit: &str,
) -> Result<(Reference, String), String> {
    match w.reference.as_ref() {
        None => Err("the reference is missing: give it inline or as a reference file".into()),
        Some(WireReference::Inline {
            freqs_hz,
            n,
            re,
            im,
        }) => {
            let (sdm, band) = inline_on_grid(w, freqs_hz, *n, re, im)?;
            let mut p = Provenance::new(Method::Breakpoints, "inline in the configuration");
            p.created_unix_secs = 0;
            let lines = sdm.lines();
            let source = format!(
                "inline, {} points from {} to {} Hz, on {} grid lines",
                freqs_hz.len(),
                freqs_hz[0],
                freqs_hz[freqs_hz.len() - 1],
                band.1 - band.0
            );
            Ok((
                Reference {
                    tolerances: Tolerances::none(lines, *n),
                    sdm,
                    df_hz: w.df_hz(),
                    band,
                    labels,
                    unit: unit.to_owned(),
                    response_transformation: None,
                    drive_transformation: None,
                    provenance: p,
                },
                source,
            ))
        }
        Some(WireReference::File { path, sha256 }) => {
            let p = std::path::PathBuf::from(path);
            let dir = if p.is_file() {
                p.parent().map(std::path::Path::to_path_buf).unwrap_or(p)
            } else {
                p
            };
            let (bin, _) = hertzforge_reference::native::paths(&dir);
            let bytes = std::fs::read(&bin)
                .map_err(|e| format!("{} could not be read ({e})", bin.display()))?;
            let hash = hertzforge_reference::native::content_hash(&bytes);
            if hash != format!("sha256:{}", sha256.to_ascii_lowercase()) {
                return Err(format!(
                    "the reference file's SHA-256 is {} but the configuration names {sha256}; \
                     choose the file again",
                    hash.trim_start_matches("sha256:")
                ));
            }
            let r = hertzforge_reference::native::read(&dir).map_err(|e| e.to_string())?;
            if (r.df_hz - w.df_hz()).abs() > 1e-9 * w.df_hz() || r.lines() != w.lines() {
                return Err(format!(
                    "the reference file has {} lines of {} Hz and the grid has {} lines of {} Hz",
                    r.lines(),
                    r.df_hz,
                    w.lines(),
                    w.df_hz()
                ));
            }
            Ok((r, format!("file {} ({hash})", dir.display())))
        }
    }
}

/// A configuration that passed its checks: the controller's configuration and
/// what was derived.
#[derive(Debug, Clone)]
pub struct Checked {
    pub core: MimoMixedConfig,
    pub derived: Derived,
}

fn law_params(r: &mut Report, field: &str, law: &RoleLaw, given: &Map<String, Value>) {
    for (k, v) in given {
        match law.params.iter().find(|p| &p.key == k) {
            None => r.errors.push(Item::field(
                "param_unknown",
                format!("{field}.params.{k}"),
                format!("{} declares no parameter \"{k}\".", law.name),
            )),
            Some(spec) => {
                if let Some(why) = crate::shock::config::param_problem(spec, v) {
                    r.errors.push(Item::field(
                        "param_out_of_range",
                        format!("{field}.params.{k}"),
                        format!("{}'s {} {why}", law.name, spec.label),
                    ));
                }
            }
        }
    }
}

/// Check `w` against `table` and the catalog. Every problem is an item of the
/// report; the configuration is `Some` only when there is none.
pub fn check(
    w: &WireMimoMixed,
    table: &ChannelTable,
    cat: &LawCatalog,
) -> (Report, Option<Checked>) {
    let mut r = Report::default();
    if !(w.fs_hz.is_finite() && w.fs_hz > 0.0) {
        r.errors.push(Item::field(
            "out_of_range",
            "estimation.fs_hz",
            "The sample rate is a number of Hz above 0.",
        ));
    }
    if w.n_pts < 64 || !w.n_pts.is_power_of_two() {
        r.errors.push(Item::field(
            "out_of_range",
            "estimation.n_pts",
            "The points per frame are a power of two from 64.",
        ));
    }
    if !r.errors.is_empty() {
        return (r, None);
    }
    // The controls' labels and unit, from the table.
    let controls: Vec<u32> = w
        .core
        .get("controls")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .map(|x| x as u32)
                .collect()
        })
        .unwrap_or_default();
    let row = |i: u32| table.channels.iter().find(|c| c.index == i);
    let labels: Vec<String> = controls
        .iter()
        .map(|i| {
            row(*i).map_or_else(
                || format!("channel {i}"),
                |c| {
                    if c.description.is_empty() {
                        format!("channel {i}")
                    } else {
                        c.description.clone()
                    }
                },
            )
        })
        .collect();
    let unit = controls.first().and_then(|i| row(*i)).map_or_else(
        || "g".to_owned(),
        |c| crate::table::unit_name(&c.units).to_string(),
    );
    let (reference, source) = match reference_on_grid(w, labels, &unit) {
        Ok(x) => x,
        Err(e) => {
            r.errors.push(Item::field(
                "reference_invalid",
                "reference",
                format!("The reference: {e}."),
            ));
            return (r, None);
        }
    };
    let mut core = w.core.clone();
    // The reference joins the controller's configuration through its serde form;
    // its tolerance lines are NaN off the band, which JSON cannot carry, so they
    // travel as zeros and are set from the floor's bands once it is read.
    let mut carried = reference.clone();
    for t in [
        &mut carried.tolerances.alarm_upper,
        &mut carried.tolerances.alarm_lower,
        &mut carried.tolerances.abort_upper,
        &mut carried.tolerances.abort_lower,
    ] {
        t.iter_mut().for_each(|x| *x = 0.0);
    }
    match serde_json::to_value(&carried) {
        Ok(v) => {
            core.insert("reference".into(), v);
        }
        Err(e) => {
            r.errors
                .push(Item::field("reference_invalid", "reference", e.to_string()));
            return (r, None);
        }
    }
    let mut cfg: MimoMixedConfig = match serde_json::from_value(Value::Object(core)) {
        Ok(c) => c,
        Err(e) => {
            r.errors.push(Item::field(
                "config_unreadable",
                "config",
                format!("The configuration could not be read: {e}."),
            ));
            return (r, None);
        }
    };
    // The reference's tolerance lines from the floor's bands (MIMO Random's form).
    let bands = floor_bands(&cfg);
    cfg.reference.tolerances = tolerances_of(&cfg.reference.sdm, cfg.reference.band, &bands);
    if let Err(e) = cfg.validate(Some(w.fs_hz)) {
        r.errors
            .push(Item::field(e.name(), "config", format!("{e}.")));
    }
    // The table: every control and limit an enabled input, every drive an enabled drive.
    let input = |i: u32| {
        table
            .channels
            .iter()
            .any(|c| c.index == i && c.enabled && c.role != ChannelRole::Drive)
    };
    for (k, c) in cfg.controls.iter().enumerate() {
        if !input(*c) {
            r.errors.push(Item::field(
                "channel_unknown",
                format!("controls[{k}]"),
                format!("Channel {c} is not an enabled input of the table."),
            ));
        }
    }
    for (k, l) in cfg.limits.iter().enumerate() {
        if !input(l.channel_index) {
            r.errors.push(Item::field(
                "channel_unknown",
                format!("limits[{k}]"),
                format!(
                    "Channel {} is not an enabled input of the table.",
                    l.channel_index
                ),
            ));
        }
    }
    for (k, d) in cfg.drives.iter().enumerate() {
        let ok = table
            .channels
            .iter()
            .any(|c| c.index == d.channel_index && c.enabled && c.role == ChannelRole::Drive);
        if let Some(first) = cfg.drives[..k]
            .iter()
            .position(|e| e.channel_index == d.channel_index)
        {
            r.errors.push(Item::field(
                "duplicate_drive",
                format!("drives[{k}]"),
                format!(
                    "Channel {} is already drive {first}; each drive is its own table row.",
                    d.channel_index
                ),
            ));
        }
        if !ok {
            r.errors.push(Item::field(
                "channel_unknown",
                format!("drives[{k}]"),
                format!(
                    "Channel {} is not an enabled drive of the table.",
                    d.channel_index
                ),
            ));
        }
    }
    // The laws, each from its own world's catalog path.
    let floor = floor_law(cat, &cfg.law.floor.id);
    let tones = tone_law(cat, &cfg.law.tones.id);
    if let Err((why, words)) = &floor {
        r.errors
            .push(Item::field(why, "law.floor.id", words.clone()));
    }
    if let Err((why, words)) = &tones {
        r.errors
            .push(Item::field(why, "law.tones.id", words.clone()));
    }
    if let Ok(f) = &floor {
        law_params(&mut r, "law.floor", f, &cfg.law.floor.params);
    }
    if let Ok(t) = &tones {
        law_params(&mut r, "law.tones", t, &cfg.law.tones.params);
        for (i, p, tone) in cfg.tones() {
            let tt = hertzforge_mimo_mixed::service::tone_test_type(&tone.schedule).as_str();
            if !t.test_types.contains(&tt) {
                r.errors.push(Item::field(
                    "law_refuses_tone",
                    format!("parts[{i}]"),
                    format!(
                        "The tone law {} does not run a {tt}, which tone \"{}\" asks of it.",
                        t.name, p.id
                    ),
                ));
            }
        }
    }
    if !r.errors.is_empty() {
        return (r, None);
    }
    let (Ok(floor), Ok(tones)) = (floor, tones) else {
        return (r, None);
    };
    let n = cfg.n();
    let (lo, hi) = cfg.reference.band;
    let df = w.df_hz();
    let reference_rms = (0..n)
        .map(|i| {
            ((lo..hi)
                .map(|l| cfg.reference.sdm.get(l, i, i).re)
                .sum::<f64>()
                * df)
                .sqrt()
        })
        .collect();
    let derived = Derived {
        fs_hz: w.fs_hz,
        n_pts: w.n_pts,
        df_hz: df,
        lines: w.lines(),
        band: cfg.reference.band,
        reference_rms,
        law: LawFacts { floor, tones },
        source,
    };
    (r, Some(Checked { core: cfg, derived }))
}

/// `law_not_checked` for a role on a hardware station.
pub fn law_not_checked(law: &RoleLaw) -> MethodError {
    let why = law
        .not_cleared
        .as_deref()
        .map_or_else(String::new, |w| format!(" ({w})"));
    MethodError::named(
        ErrorCode::Refused,
        "law_not_checked",
        format!(
            "The {} law \"{}\" is not cleared for hardware in MIMO Mixed Mode{why}; it needs a \
             passed check in its own world and a passed MIMO Mixed check in this role.",
            if law.role == LawRole::Floor {
                "floor"
            } else {
                "tone"
            },
            law.name
        ),
    )
    .with_detail(json!({"role": role_name(law.role), "id": law.id}))
}

/// The run's laws from the catalog: the floor's through MIMO Random's path, the
/// tones' through MIMO Sine's, each with its parameters. On `hardware` each
/// role's law is looked up afresh and must clear the role's rule.
pub fn resolve_laws(
    cat: &LawCatalog,
    cfg: &MimoMixedConfig,
    hardware: bool,
) -> Result<MimoMixedLaws, MethodError> {
    let refused = |role: LawRole, words: String| {
        MethodError::named(ErrorCode::Refused, "arm_refused", words)
            .with_detail(json!({"role": role_name(role)}))
    };
    let floor_facts =
        floor_law(cat, &cfg.law.floor.id).map_err(|(_, w)| refused(LawRole::Floor, w))?;
    let tone_facts =
        tone_law(cat, &cfg.law.tones.id).map_err(|(_, w)| refused(LawRole::Tones, w))?;
    if hardware {
        for f in [&floor_facts, &tone_facts] {
            if !f.hardware {
                return Err(law_not_checked(f));
            }
        }
    }
    let pick = &cfg.law.floor;
    let floor = if let Some(b) = BuiltinMimo::from_id(&pick.id) {
        let specs = builtin_mimo_manifest(b).law.params;
        let choice = LawChoice::new(
            pick.id.clone(),
            crate::mimo::adapter::params_of(&specs, &pick.params),
        );
        hertzforge_mimo_random::law::builtin_factory(&choice, cfg.n()).map_err(|e| {
            refused(
                LawRole::Floor,
                format!("The floor law could not be built: {e}."),
            )
        })?
    } else {
        let choice = LawChoice::new(
            pick.id.clone(),
            crate::mimo::adapter::params_of(&floor_facts.params, &pick.params),
        );
        cat.resolve_mimo(&choice, cfg.n()).map_err(|e| {
            refused(
                LawRole::Floor,
                format!("The floor law could not be built: {e}."),
            )
        })?
    };
    let budget = Duration::from_secs_f64(
        (hertzforge_mimo_sine::setup::GuardSettings::default().law_budget * cfg.control_tick_s())
            .max(1e-4),
    );
    let tones = crate::mimo_sine::laws::resolve(
        cat,
        &cfg.law.tones.id,
        &cfg.law.tones.params,
        budget,
        BUILD_BUDGET,
        false,
    )
    .map_err(|e| {
        let detail = match e.detail.as_deref() {
            Some(Value::Object(m)) => {
                let mut m = m.clone();
                m.insert("role".into(), Value::from("tones"));
                Value::Object(m)
            }
            _ => json!({"role": "tones"}),
        };
        e.with_detail(detail)
    })?;
    Ok(MimoMixedLaws { floor, tones })
}

/// The parts' kinds, ids and names, in order.
pub fn part_kind(b: &MimoPartBody) -> &'static str {
    match b {
        MimoPartBody::Floor(_) => "floor",
        MimoPartBody::Tone(_) => "tone",
        MimoPartBody::Narrowband(_) => "narrowband",
        _ => "unknown",
    }
}
