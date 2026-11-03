//! Conversions between the host's plain types and the types `bindgen!`
//! generates from the WIT. Plain to WIT copies the caller's slices into owned
//! lists (they are copied into the law's memory anyway); WIT to plain widens
//! line numbers to `usize`.

use crate::bindings::hertzforge::control_law::types as wit;
use crate::error::LawError;
use crate::types::{
    Breakpoint, ChoiceSpec, CurveSpec, DiagnosticSpec, DiagnosticValue, IntegerSpec,
    LoopDiagnostics, LoopInputs, LoopOutputs, Manifest, NumberSpec, Param, ParamKind, ParamSpec,
    ParamValue, Setup,
};

fn number_spec(s: wit::NumberSpec) -> NumberSpec {
    NumberSpec {
        unit: s.unit,
        min: s.min,
        max: s.max,
        step: s.step,
        default: s.default,
    }
}

fn breakpoint_from(b: &wit::Breakpoint) -> Breakpoint {
    Breakpoint {
        frequency_hz: b.frequency_hz,
        value: b.value,
    }
}

fn breakpoint_to(b: &Breakpoint) -> wit::Breakpoint {
    wit::Breakpoint {
        frequency_hz: b.frequency_hz,
        value: b.value,
    }
}

fn param_kind(k: wit::ParamKind) -> ParamKind {
    match k {
        wit::ParamKind::Number(s) => ParamKind::Number(number_spec(s)),
        wit::ParamKind::Integer(s) => ParamKind::Integer(IntegerSpec {
            unit: s.unit,
            min: s.min,
            max: s.max,
            step: s.step,
            default: s.default,
        }),
        wit::ParamKind::Toggle(d) => ParamKind::Toggle(d),
        wit::ParamKind::Choice(s) => ParamKind::Choice(ChoiceSpec {
            options: s.options,
            default: s.default,
        }),
        wit::ParamKind::Curve(s) => ParamKind::Curve(CurveSpec {
            unit: s.unit,
            min: s.min,
            max: s.max,
            default: s.default.iter().map(breakpoint_from).collect(),
        }),
        wit::ParamKind::PerChannel(s) => ParamKind::PerChannel(number_spec(s)),
    }
}

/// A manifest as the law returned it, unchecked.
pub(crate) fn manifest(m: wit::Manifest) -> Manifest {
    Manifest {
        name: m.name,
        version: m.version,
        author: m.author,
        summary: m.summary,
        params: m
            .params
            .into_iter()
            .map(|p| ParamSpec {
                key: p.key,
                label: p.label,
                help: p.help,
                kind: param_kind(p.kind),
            })
            .collect(),
        diagnostics: m
            .diagnostics
            .into_iter()
            .map(|d| DiagnosticSpec {
                key: d.key,
                label: d.label,
                unit: d.unit,
                help: d.help,
            })
            .collect(),
    }
}

pub(crate) fn params(ps: &[Param]) -> Vec<wit::Param> {
    ps.iter()
        .map(|p| wit::Param {
            key: p.key.clone(),
            value: match &p.value {
                ParamValue::Number(v) => wit::ParamValue::Number(*v),
                ParamValue::Integer(v) => wit::ParamValue::Integer(*v),
                ParamValue::Toggle(v) => wit::ParamValue::Toggle(*v),
                ParamValue::Choice(v) => wit::ParamValue::Choice(v.clone()),
                ParamValue::Curve(v) => {
                    wit::ParamValue::Curve(v.iter().map(breakpoint_to).collect())
                }
                ParamValue::PerChannel(v) => wit::ParamValue::PerChannel(v.clone()),
            },
        })
        .collect()
}

fn band(b: (usize, usize)) -> Result<wit::Band, String> {
    let lo = u32::try_from(b.0).map_err(|_| format!("band start {} is too large", b.0))?;
    let hi = u32::try_from(b.1).map_err(|_| format!("band end {} is too large", b.1))?;
    Ok(wit::Band { lo, hi })
}

/// A checked setup (`validate_setup` passed).
pub(crate) fn setup(s: &Setup<'_>) -> Result<wit::Setup, LawError> {
    Ok(wit::Setup {
        target_psd: s.target_psd.to_vec(),
        df_hz: s.df_hz,
        band: band(s.band).map_err(LawError::SetupInvalid)?,
        control_channels: u32::try_from(s.control_channels).map_err(|_| {
            LawError::SetupInvalid(format!("{} control channels", s.control_channels))
        })?,
    })
}

/// Checked inputs (`validate_inputs` passed).
pub(crate) fn inputs(i: &LoopInputs<'_>) -> Result<wit::LoopInputs, LawError> {
    Ok(wit::LoopInputs {
        target_psd: i.target_psd.to_vec(),
        measured_psd: i.measured_psd.to_vec(),
        h_mag2: i.h_mag2.to_vec(),
        coherence: i.coherence.to_vec(),
        current_level_db: i.current_level_db,
        df_hz: i.df_hz,
        notched_bins: i.notched_bins.to_vec(),
        band: band(i.band).map_err(LawError::InputsInvalid)?,
        dof: i.dof,
        loop_index: i.loop_index,
    })
}

fn bins(v: Vec<u32>) -> Vec<usize> {
    // `u32` always fits `usize` on the 32- and 64-bit targets the host
    // builds for; saturate rather than panic on anything narrower, so the
    // grid check still refuses the line.
    v.into_iter()
        .map(|k| usize::try_from(k).unwrap_or(usize::MAX))
        .collect()
}

/// Outputs as the law returned them, unchecked.
pub(crate) fn outputs(o: wit::LoopOutputs) -> LoopOutputs {
    let d = o.diagnostics;
    LoopOutputs {
        drive_psd: o.drive_psd,
        level_cmd_db: o.level_cmd_db,
        diagnostics: LoopDiagnostics {
            untrusted_bins: bins(d.untrusted_bins),
            clamped_bins: bins(d.clamped_bins),
            divergent_bins: bins(d.divergent_bins),
            inband_err_db: d.inband_err_db,
            inband_rms_g: d.inband_rms_g,
            servo_faulted: d.servo_faulted,
            values: d
                .values
                .into_iter()
                .map(|v| DiagnosticValue {
                    key: v.key,
                    value: v.value,
                })
                .collect(),
        },
    }
}
