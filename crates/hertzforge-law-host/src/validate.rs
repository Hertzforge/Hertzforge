//! The rules the host enforces on what a law declares, what it is given and
//! what it returns. Pure functions over the plain types, so each rule is
//! tested without a WebAssembly component.

use crate::error::{LawError, OutputRule};
use crate::types::{
    Breakpoint, ChoiceSpec, CurveSpec, DiagnosticSpec, IntegerSpec, LoopInputs, LoopOutputs,
    Manifest, NumberSpec, Param, ParamKind, ParamSpec, ParamValue, Setup,
};
use std::collections::HashSet;

/// Longest key (parameter or diagnostic), characters.
pub const MAX_KEY_LEN: usize = 64;
/// Most parameters a law may declare.
pub const MAX_PARAMS: usize = 128;
/// Most diagnostics a law may declare.
pub const MAX_DIAGNOSTICS: usize = 128;
/// Most options a choice may offer.
pub const MAX_CHOICE_OPTIONS: usize = 64;
/// Most breakpoints a curve may hold.
pub const MAX_CURVE_POINTS: usize = 1024;
/// Most per-channel values (control channels).
pub const MAX_CONTROL_CHANNELS: usize = 1024;

const MAX_NAME_LEN: usize = 128;
const MAX_VERSION_LEN: usize = 64;
const MAX_AUTHOR_LEN: usize = 256;
const MAX_SUMMARY_LEN: usize = 4096;
const MAX_LABEL_LEN: usize = 128;
const MAX_HELP_LEN: usize = 2048;
const MAX_UNIT_LEN: usize = 32;

fn manifest_err(msg: impl Into<String>) -> LawError {
    LawError::ManifestInvalid(msg.into())
}

fn text(field: &str, value: &str, max: usize, required: bool) -> Result<(), LawError> {
    if required && value.trim().is_empty() {
        return Err(manifest_err(format!("{field} is empty")));
    }
    if value.chars().count() > max {
        return Err(manifest_err(format!(
            "{field} is longer than {max} characters"
        )));
    }
    if value.chars().any(|c| c.is_control() && c != '\n') {
        return Err(manifest_err(format!("{field} holds a control character")));
    }
    Ok(())
}

/// Whether `key` is a valid parameter or diagnostic key: `[a-z][a-z0-9_]*`,
/// at most [`MAX_KEY_LEN`] characters.
pub fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    key.len() <= MAX_KEY_LEN
        && first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn number_spec(what: &str, s: &NumberSpec) -> Result<(), LawError> {
    text(&format!("{what}: unit"), &s.unit, MAX_UNIT_LEN, false)?;
    for (name, v) in [
        ("min", s.min),
        ("max", s.max),
        ("step", s.step),
        ("default", s.default),
    ] {
        if !v.is_finite() {
            return Err(manifest_err(format!("{what}: {name} is not finite")));
        }
    }
    if s.min > s.max {
        return Err(manifest_err(format!(
            "{what}: min {} is above max {}",
            s.min, s.max
        )));
    }
    if s.step <= 0.0 {
        return Err(manifest_err(format!(
            "{what}: step must be above zero (got {})",
            s.step
        )));
    }
    if s.default < s.min || s.default > s.max {
        return Err(manifest_err(format!(
            "{what}: default {} is outside [{}, {}]",
            s.default, s.min, s.max
        )));
    }
    Ok(())
}

fn integer_spec(what: &str, s: &IntegerSpec) -> Result<(), LawError> {
    text(&format!("{what}: unit"), &s.unit, MAX_UNIT_LEN, false)?;
    if s.min > s.max {
        return Err(manifest_err(format!(
            "{what}: min {} is above max {}",
            s.min, s.max
        )));
    }
    if s.step < 1 {
        return Err(manifest_err(format!(
            "{what}: step must be at least 1 (got {})",
            s.step
        )));
    }
    if s.default < s.min || s.default > s.max {
        return Err(manifest_err(format!(
            "{what}: default {} is outside [{}, {}]",
            s.default, s.min, s.max
        )));
    }
    Ok(())
}

fn choice_spec(what: &str, s: &ChoiceSpec) -> Result<(), LawError> {
    if s.options.is_empty() || s.options.len() > MAX_CHOICE_OPTIONS {
        return Err(manifest_err(format!(
            "{what}: a choice needs 1 to {MAX_CHOICE_OPTIONS} options (got {})",
            s.options.len()
        )));
    }
    let mut seen = HashSet::new();
    for o in &s.options {
        text(&format!("{what}: option"), o, MAX_LABEL_LEN, true)?;
        if !seen.insert(o.as_str()) {
            return Err(manifest_err(format!(
                "{what}: option `{o}` is listed twice"
            )));
        }
    }
    if !seen.contains(s.default.as_str()) {
        return Err(manifest_err(format!(
            "{what}: default `{}` is not one of the options",
            s.default
        )));
    }
    Ok(())
}

/// The curve rules shared by a declared default and a supplied value: 1 to
/// [`MAX_CURVE_POINTS`] points, finite non-negative frequencies strictly
/// increasing, finite values within `[min, max]`.
fn curve_points(points: &[Breakpoint], min: f64, max: f64) -> Result<(), String> {
    if points.is_empty() || points.len() > MAX_CURVE_POINTS {
        return Err(format!(
            "a curve needs 1 to {MAX_CURVE_POINTS} points (got {})",
            points.len()
        ));
    }
    let mut last: Option<f64> = None;
    for (i, p) in points.iter().enumerate() {
        if !p.frequency_hz.is_finite() || p.frequency_hz < 0.0 {
            return Err(format!(
                "point {i}: frequency {} is not a finite, non-negative number",
                p.frequency_hz
            ));
        }
        if let Some(prev) = last {
            if p.frequency_hz <= prev {
                return Err(format!(
                    "point {i}: frequency {} does not increase on {prev}",
                    p.frequency_hz
                ));
            }
        }
        last = Some(p.frequency_hz);
        if !p.value.is_finite() || p.value < min || p.value > max {
            return Err(format!(
                "point {i}: value {} is outside [{min}, {max}]",
                p.value
            ));
        }
    }
    Ok(())
}

fn curve_spec(what: &str, s: &CurveSpec) -> Result<(), LawError> {
    text(&format!("{what}: unit"), &s.unit, MAX_UNIT_LEN, false)?;
    if !s.min.is_finite() || !s.max.is_finite() {
        return Err(manifest_err(format!("{what}: min or max is not finite")));
    }
    if s.min > s.max {
        return Err(manifest_err(format!(
            "{what}: min {} is above max {}",
            s.min, s.max
        )));
    }
    curve_points(&s.default, s.min, s.max)
        .map_err(|e| manifest_err(format!("{what}: default curve: {e}")))
}

fn param_spec(p: &ParamSpec) -> Result<(), LawError> {
    let what = format!("parameter `{}`", p.key);
    text(&format!("{what}: label"), &p.label, MAX_LABEL_LEN, true)?;
    text(&format!("{what}: help"), &p.help, MAX_HELP_LEN, false)?;
    match &p.kind {
        ParamKind::Number(s) | ParamKind::PerChannel(s) => number_spec(&what, s),
        ParamKind::Integer(s) => integer_spec(&what, s),
        ParamKind::Toggle(_) => Ok(()),
        ParamKind::Choice(s) => choice_spec(&what, s),
        ParamKind::Curve(s) => curve_spec(&what, s),
    }
}

fn diagnostic_spec(d: &DiagnosticSpec) -> Result<(), LawError> {
    let what = format!("diagnostic `{}`", d.key);
    text(&format!("{what}: label"), &d.label, MAX_LABEL_LEN, true)?;
    text(&format!("{what}: unit"), &d.unit, MAX_UNIT_LEN, false)?;
    text(&format!("{what}: help"), &d.help, MAX_HELP_LEN, false)
}

/// Check a manifest: a non-empty name and version, bounded text, valid and
/// unique keys, and every parameter's limits and default consistent.
pub fn validate_manifest(m: &Manifest) -> Result<(), LawError> {
    text("name", &m.name, MAX_NAME_LEN, true)?;
    text("version", &m.version, MAX_VERSION_LEN, true)?;
    text("author", &m.author, MAX_AUTHOR_LEN, false)?;
    text("summary", &m.summary, MAX_SUMMARY_LEN, false)?;
    if m.params.len() > MAX_PARAMS {
        return Err(manifest_err(format!(
            "{} parameters declared; at most {MAX_PARAMS}",
            m.params.len()
        )));
    }
    if m.diagnostics.len() > MAX_DIAGNOSTICS {
        return Err(manifest_err(format!(
            "{} diagnostics declared; at most {MAX_DIAGNOSTICS}",
            m.diagnostics.len()
        )));
    }
    let mut keys = HashSet::new();
    for p in &m.params {
        if !is_valid_key(&p.key) {
            return Err(manifest_err(format!(
                "parameter key `{}` is not of the form [a-z][a-z0-9_]* (at most {MAX_KEY_LEN} \
                 characters)",
                p.key
            )));
        }
        if !keys.insert(p.key.as_str()) {
            return Err(manifest_err(format!(
                "parameter key `{}` is declared twice",
                p.key
            )));
        }
        param_spec(p)?;
    }
    let mut diag_keys = HashSet::new();
    for d in &m.diagnostics {
        if !is_valid_key(&d.key) {
            return Err(manifest_err(format!(
                "diagnostic key `{}` is not of the form [a-z][a-z0-9_]* (at most {MAX_KEY_LEN} \
                 characters)",
                d.key
            )));
        }
        if !diag_keys.insert(d.key.as_str()) {
            return Err(manifest_err(format!(
                "diagnostic key `{}` is declared twice",
                d.key
            )));
        }
        diagnostic_spec(d)?;
    }
    Ok(())
}

fn number_in(s: &NumberSpec, v: f64) -> Result<(), String> {
    if !v.is_finite() {
        return Err(format!("{v} is not a finite number"));
    }
    if v < s.min || v > s.max {
        return Err(format!("{v} is outside [{}, {}]", s.min, s.max));
    }
    Ok(())
}

fn kind_name(v: &ParamValue) -> &'static str {
    match v {
        ParamValue::Number(_) => "a number",
        ParamValue::Integer(_) => "an integer",
        ParamValue::Toggle(_) => "a toggle",
        ParamValue::Choice(_) => "a choice",
        ParamValue::Curve(_) => "a curve",
        ParamValue::PerChannel(_) => "a per-channel list",
    }
}

fn value_fits(kind: &ParamKind, value: &ParamValue, channels: usize) -> Result<(), String> {
    match (kind, value) {
        (ParamKind::Number(s), ParamValue::Number(v)) => number_in(s, *v),
        (ParamKind::Integer(s), ParamValue::Integer(v)) => {
            if *v < s.min || *v > s.max {
                Err(format!("{v} is outside [{}, {}]", s.min, s.max))
            } else {
                Ok(())
            }
        }
        (ParamKind::Toggle(_), ParamValue::Toggle(_)) => Ok(()),
        (ParamKind::Choice(s), ParamValue::Choice(v)) => {
            if s.options.iter().any(|o| o == v) {
                Ok(())
            } else {
                Err(format!(
                    "`{v}` is not one of the options ({})",
                    s.options.join(", ")
                ))
            }
        }
        (ParamKind::Curve(s), ParamValue::Curve(points)) => curve_points(points, s.min, s.max),
        (ParamKind::PerChannel(s), ParamValue::PerChannel(vs)) => {
            if vs.len() != channels {
                return Err(format!(
                    "{} values for {channels} control channels",
                    vs.len()
                ));
            }
            for (i, v) in vs.iter().enumerate() {
                number_in(s, *v).map_err(|e| format!("channel {i}: {e}"))?;
            }
            Ok(())
        }
        (kind, value) => Err(format!(
            "expected {}, got {}",
            match kind {
                ParamKind::Number(_) => "a number",
                ParamKind::Integer(_) => "an integer",
                ParamKind::Toggle(_) => "a toggle",
                ParamKind::Choice(_) => "a choice",
                ParamKind::Curve(_) => "a curve",
                ParamKind::PerChannel(_) => "a per-channel list",
            },
            kind_name(value)
        )),
    }
}

fn default_value(kind: &ParamKind, channels: usize) -> ParamValue {
    match kind {
        ParamKind::Number(s) => ParamValue::Number(s.default),
        ParamKind::Integer(s) => ParamValue::Integer(s.default),
        ParamKind::Toggle(d) => ParamValue::Toggle(*d),
        ParamKind::Choice(s) => ParamValue::Choice(s.default.clone()),
        ParamKind::Curve(s) => ParamValue::Curve(s.default.clone()),
        ParamKind::PerChannel(s) => ParamValue::PerChannel(vec![s.default; channels]),
    }
}

/// Check supplied parameter values against a (valid) manifest and complete
/// them: every key must be declared and supplied at most once, and every
/// value must be of the declared kind and within its limits. Returns one
/// value per declared parameter, in manifest order, with defaults for the
/// ones not supplied.
pub fn resolve_params(
    manifest: &Manifest,
    supplied: &[Param],
    control_channels: usize,
) -> Result<Vec<Param>, LawError> {
    let mut seen = HashSet::new();
    for p in supplied {
        let Some(spec) = manifest.params.iter().find(|s| s.key == p.key) else {
            return Err(LawError::ParamInvalid {
                key: p.key.clone(),
                reason: "not declared by this law".into(),
            });
        };
        if !seen.insert(p.key.as_str()) {
            return Err(LawError::ParamInvalid {
                key: p.key.clone(),
                reason: "supplied twice".into(),
            });
        }
        value_fits(&spec.kind, &p.value, control_channels).map_err(|reason| {
            LawError::ParamInvalid {
                key: p.key.clone(),
                reason,
            }
        })?;
    }
    Ok(manifest
        .params
        .iter()
        .map(|spec| {
            supplied
                .iter()
                .find(|p| p.key == spec.key)
                .cloned()
                .unwrap_or_else(|| Param {
                    key: spec.key.clone(),
                    value: default_value(&spec.kind, control_channels),
                })
        })
        .collect())
}

fn spectrum(name: &str, v: &[f64], n: usize) -> Result<(), String> {
    if v.len() != n {
        return Err(format!("{name} has {} lines, the grid has {n}", v.len()));
    }
    if let Some((k, x)) = v
        .iter()
        .enumerate()
        .find(|(_, x)| !x.is_finite() || **x < 0.0)
    {
        return Err(format!(
            "{name} line {k} is {x}, not a finite, non-negative number"
        ));
    }
    Ok(())
}

fn band(band: (usize, usize), n: usize) -> Result<(), String> {
    let (lo, hi) = band;
    if lo >= hi || hi > n {
        return Err(format!(
            "band [{lo}, {hi}) does not fit a grid of {n} lines"
        ));
    }
    Ok(())
}

/// Check a setup: a non-empty grid that fits the interface's 32-bit line
/// numbers, a finite non-negative target, a positive line spacing, a band on
/// the grid and 1 to [`MAX_CONTROL_CHANNELS`] control channels.
pub fn validate_setup(s: &Setup<'_>) -> Result<(), LawError> {
    let n = s.target_psd.len();
    let fail = |m: String| Err(LawError::SetupInvalid(m));
    if n == 0 {
        return fail("the target grid is empty".into());
    }
    if u32::try_from(n).is_err() {
        return fail(format!("{n} lines is more than the interface carries"));
    }
    if let Err(m) = spectrum("target", s.target_psd, n) {
        return fail(m);
    }
    if !s.df_hz.is_finite() || s.df_hz <= 0.0 {
        return fail(format!(
            "line spacing {} Hz is not a positive number",
            s.df_hz
        ));
    }
    if let Err(m) = band(s.band, n) {
        return fail(m);
    }
    if s.control_channels == 0 || s.control_channels > MAX_CONTROL_CHANNELS {
        return fail(format!(
            "{} control channels; 1 to {MAX_CONTROL_CHANNELS} are accepted",
            s.control_channels
        ));
    }
    Ok(())
}

/// Check one loop's inputs against the grid of `n` lines the law was built
/// for. These are the caller's numbers: a failure here is never the law's.
pub fn validate_inputs(i: &LoopInputs<'_>, n: usize) -> Result<(), LawError> {
    let check = || -> Result<(), String> {
        spectrum("target", i.target_psd, n)?;
        spectrum("measured", i.measured_psd, n)?;
        spectrum("plant gain", i.h_mag2, n)?;
        spectrum("coherence", i.coherence, n)?;
        if !i.notched_bins.is_empty() && i.notched_bins.len() != n {
            return Err(format!(
                "the notch mask has {} lines, the grid has {n}",
                i.notched_bins.len()
            ));
        }
        band(i.band, n)?;
        if !i.df_hz.is_finite() || i.df_hz <= 0.0 {
            return Err(format!(
                "line spacing {} Hz is not a positive number",
                i.df_hz
            ));
        }
        if !i.current_level_db.is_finite() {
            return Err(format!(
                "the current level {} dB is not finite",
                i.current_level_db
            ));
        }
        if !i.dof.is_finite() || i.dof < 0.0 {
            return Err(format!(
                "degrees of freedom {} is not a finite, non-negative number",
                i.dof
            ));
        }
        Ok(())
    };
    check().map_err(LawError::InputsInvalid)
}

/// Check one loop's outputs: a drive of `n` finite, non-negative lines, a
/// finite level command, reported lines on the grid, finite optional values,
/// and named values that are declared, unique and finite. A failure is
/// [`LawError::BadOutput`] naming the first rule broken, in [`OutputRule`]
/// order.
pub fn validate_outputs(
    o: &LoopOutputs,
    n: usize,
    diagnostics: &[DiagnosticSpec],
) -> Result<(), LawError> {
    let bad = |rule: OutputRule, detail: String| Err(LawError::BadOutput { rule, detail });
    if o.drive_psd.len() != n {
        return bad(
            OutputRule::WrongLength,
            format!(
                "the drive has {} lines, the grid has {n}",
                o.drive_psd.len()
            ),
        );
    }
    if let Some((k, x)) = o.drive_psd.iter().enumerate().find(|(_, x)| !x.is_finite()) {
        return bad(
            OutputRule::NonFiniteDrive,
            format!("drive line {k} is {x}, not a finite number"),
        );
    }
    if let Some((k, x)) = o.drive_psd.iter().enumerate().find(|(_, x)| **x < 0.0) {
        return bad(
            OutputRule::NegativeDrive,
            format!("drive line {k} is {x}, below zero"),
        );
    }
    if !o.level_cmd_db.is_finite() {
        return bad(
            OutputRule::NonFiniteLevel,
            format!("the level command {} dB is not finite", o.level_cmd_db),
        );
    }
    let d = &o.diagnostics;
    let diag = |detail: String| bad(OutputRule::Diagnostics, detail);
    for (name, bins) in [
        ("untrusted", &d.untrusted_bins),
        ("clamped", &d.clamped_bins),
        ("divergent", &d.divergent_bins),
    ] {
        if bins.len() > n {
            return diag(format!(
                "{} {name} lines reported on a grid of {n}",
                bins.len()
            ));
        }
        if let Some(k) = bins.iter().find(|k| **k >= n) {
            return diag(format!("{name} line {k} is off the grid of {n}"));
        }
    }
    for (name, v) in [
        ("in-band error", d.inband_err_db),
        ("in-band RMS", d.inband_rms_g),
    ] {
        if let Some(x) = v {
            if !x.is_finite() {
                return diag(format!("{name} {x} is not finite"));
            }
        }
    }
    let mut seen = HashSet::new();
    for v in &d.values {
        if !diagnostics.iter().any(|s| s.key == v.key) {
            return diag(format!(
                "diagnostic `{}` is not declared in the manifest",
                v.key
            ));
        }
        if !seen.insert(v.key.as_str()) {
            return diag(format!("diagnostic `{}` is reported twice", v.key));
        }
        if !v.value.is_finite() {
            return diag(format!("diagnostic `{}` is {}, not finite", v.key, v.value));
        }
    }
    Ok(())
}
