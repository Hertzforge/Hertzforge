//! The law's options and their `name:value` text form.

use std::collections::HashMap;

/// How each band's threshold kappa is chosen from the sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KappaRule {
    /// The bands are chosen together along the error-energy front.
    #[default]
    Front,
    /// The smallest kappa within 0.1 dB of the band's best error.
    Knee,
    /// The corner of each band's L-curve.
    LCurve,
}

/// Whether the band solves are regularized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Regularization {
    /// Automatic threshold from the sweep, or `condition_number` when given.
    #[default]
    Auto,
    /// The plain pseudoinverse.
    None,
}

/// How an accepted FRF update enters the band sensitivity.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TfUpdateBlend {
    /// Each update is weighted as one new frame of information.
    #[default]
    Cumulative,
    /// Exponential forgetting with this weight on the update.
    Beta(f64),
}

/// Every option of the law, with the Python defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// Frequency resolution [Hz]. Required by the Python; `default()` leaves it
    /// at 0, which `control` refuses.
    pub df: f64,
    /// 1/N-octave bands.
    pub oct_order: usize,
    /// How each band's kappa is chosen from the sweep.
    pub kappa_rule: KappaRule,
    /// Fraction of the selected solution's predicted drive energy to give up.
    pub drive_reduction: f64,
    /// Automatic or no regularization.
    pub regularization: Regularization,
    /// Re-solve from the caller's running FRF estimate.
    pub use_tf_update: bool,
    /// kappa_d: per-band bound on the emitted drive CPSD condition number.
    pub drive_condition_number: f64,
    /// alpha: minimum per-drive autopower as a fraction of the band-average power.
    pub min_drive_fraction: f64,
    /// Leave a band out of the FRF update when its conditioning load was capped.
    pub hold_capped_bands: bool,
    /// Multiple coherence below which a channel's line is left out of the FRF update.
    pub min_coherence: Option<f64>,
    /// How accepted updates are blended.
    pub tf_update_blend: TfUpdateBlend,
    /// Override of the cumulative blend's seed count.
    pub n_e_seed: Option<f64>,
    /// 1-based channel numbers treated as narrowband response limits.
    pub limit_channel_indices: Vec<usize>,
    /// An explicit threshold kappa, bypassing the sweep.
    pub condition_number: Option<f64>,
    /// Re-fit over PSD drives where dropping negative eigenvalues moves a response.
    pub constrained_solve: bool,
    /// Per-band, per-channel error feedback from the measured responses.
    pub gain_correction: bool,
    /// Exponential smoothing rate of the gain correction per call.
    pub gain_correction_rate: f64,
    /// Channel check threshold [dB]; `None` turns it off.
    pub channel_check_db: Option<f64>,
    /// RMS limit per drive, one value for all or one per drive; `None` turns it off.
    pub drive_rms_limit: Option<Vec<f64>>,
    /// Also target the specification's cross terms. `use_buzz` implies this.
    pub control_to_cross_terms: bool,
    /// Take the cross terms from the system identification's response CPSD.
    pub use_buzz: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            df: 0.0,
            oct_order: 6,
            kappa_rule: KappaRule::Front,
            drive_reduction: 0.0,
            regularization: Regularization::Auto,
            use_tf_update: false,
            drive_condition_number: 100.0,
            min_drive_fraction: 0.02,
            hold_capped_bands: false,
            min_coherence: None,
            tf_update_blend: TfUpdateBlend::Cumulative,
            n_e_seed: None,
            limit_channel_indices: Vec::new(),
            condition_number: None,
            constrained_solve: true,
            gain_correction: true,
            gain_correction_rate: 0.25,
            channel_check_db: Some(20.0),
            drive_rms_limit: Some(vec![1.5]),
            control_to_cross_terms: false,
            use_buzz: false,
        }
    }
}

fn number(name: &str, value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{name}: '{value}' is not a number"))
}

fn is_true(value: &str) -> bool {
    matches!(
        value.trim().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

impl Params {
    /// Parses the newline-separated `name:value` form. Names are case
    /// insensitive and unknown names are ignored, as in the Python. `df` is required.
    pub fn parse(text: &str) -> Result<Params, String> {
        let mut map: HashMap<String, String> = HashMap::new();
        for line in text.lines() {
            if let Some((name, value)) = line.trim().split_once(':') {
                map.insert(name.trim().to_lowercase(), value.trim().to_string());
            }
        }
        let get = |name: &str| map.get(name).map(String::as_str);
        let float = |name: &str, default: f64| match get(name) {
            Some(v) => number(name, v),
            None => Ok(default),
        };
        let flag = |name: &str, default: bool| get(name).map_or(default, is_true);
        let optional =
            |name: &str, off: &[&str], default: Option<f64>| -> Result<Option<f64>, String> {
                match get(name) {
                    None => Ok(default),
                    Some(v) if off.contains(&v.to_lowercase().as_str()) => Ok(None),
                    Some(v) => number(name, v).map(Some),
                }
            };

        let d = Params::default();
        let df = number("df", get("df").ok_or("df is required")?)?;
        let oct_order = match get("oct_order") {
            Some(v) => v
                .parse()
                .map_err(|_| format!("oct_order: '{v}' is not a positive integer"))?,
            None => d.oct_order,
        };
        if oct_order == 0 {
            return Err("oct_order must be positive".into());
        }
        let kappa_rule = match get("kappa_rule").unwrap_or("front").to_lowercase().as_str() {
            "front" => KappaRule::Front,
            "knee" => KappaRule::Knee,
            "lcurve" => KappaRule::LCurve,
            other => {
                return Err(format!(
                    "kappa_rule must be 'knee', 'front' or 'lcurve', not '{other}'"
                ))
            }
        };
        let regularization = match get("regularization")
            .unwrap_or("auto")
            .to_lowercase()
            .as_str()
        {
            "auto" => Regularization::Auto,
            "none" => Regularization::None,
            other => {
                return Err(format!(
                    "regularization must be 'auto' or 'none', not '{other}'"
                ))
            }
        };
        let tf_update_blend = match get("tf_update_blend").map(str::to_lowercase) {
            None => TfUpdateBlend::Cumulative,
            Some(v) if v == "cumulative" => TfUpdateBlend::Cumulative,
            Some(v) => TfUpdateBlend::Beta(number("tf_update_blend", &v)?),
        };
        let drive_rms_limit = match get("drive_rms_limit").map(str::to_lowercase) {
            None => d.drive_rms_limit,
            Some(v) if matches!(v.as_str(), "none" | "off" | "inf" | "") => None,
            Some(v) => Some(
                v.split(',')
                    .map(|x| number("drive_rms_limit", x))
                    .collect::<Result<Vec<f64>, String>>()?,
            ),
        };
        let limit_channel_indices = match get("limit_channel_indices") {
            None | Some("") => Vec::new(),
            Some(v) => v
                .split(',')
                .map(|x| match x.trim().parse::<usize>() {
                    Ok(i) if i >= 1 => Ok(i),
                    _ => Err(format!(
                        "limit_channel_indices: '{x}' is not a 1-based channel number"
                    )),
                })
                .collect::<Result<Vec<usize>, String>>()?,
        };
        let use_buzz = flag("use_buzz", d.use_buzz);
        Ok(Params {
            df,
            oct_order,
            kappa_rule,
            drive_reduction: float("drive_reduction", d.drive_reduction)?,
            regularization,
            use_tf_update: flag("use_tf_update", d.use_tf_update),
            drive_condition_number: float("drive_condition_number", d.drive_condition_number)?,
            min_drive_fraction: float("min_drive_fraction", d.min_drive_fraction)?,
            hold_capped_bands: flag("hold_capped_bands", d.hold_capped_bands),
            min_coherence: optional("min_coherence", &["none", "off", ""], None)?,
            tf_update_blend,
            n_e_seed: get("n_e_seed").map(|v| number("n_e_seed", v)).transpose()?,
            limit_channel_indices,
            condition_number: get("condition_number")
                .map(|v| number("condition_number", v))
                .transpose()?,
            constrained_solve: flag("constrained_solve", d.constrained_solve),
            gain_correction: flag("gain_correction", d.gain_correction),
            gain_correction_rate: float("gain_correction_rate", d.gain_correction_rate)?,
            channel_check_db: optional(
                "channel_check_db",
                &["none", "off", "inf", ""],
                Some(20.0),
            )?,
            drive_rms_limit,
            control_to_cross_terms: use_buzz
                || flag("control_to_cross_terms", d.control_to_cross_terms),
            use_buzz,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_study_form_parses() {
        let p =
            Params::parse("df:1.0\ndrive_rms_limit:none\ndrive_reduction:0.70\nuse_tf_update:True")
                .unwrap();
        assert_eq!(p.df, 1.0);
        assert_eq!(p.drive_rms_limit, None);
        assert_eq!(p.drive_reduction, 0.70);
        assert!(p.use_tf_update);
        assert!(p.gain_correction);
        assert_eq!(p.kappa_rule, KappaRule::Front);
    }

    #[test]
    fn every_other_option_parses() {
        let p = Params::parse(
            " DF : 2.5 \noct_order:3\nkappa_rule:KNEE\nregularization:none\ndrive_condition_number:50\n\
             min_drive_fraction:0.1\nhold_capped_bands:yes\nmin_coherence:0.8\ntf_update_blend:0.2\n\
             n_e_seed:10\nlimit_channel_indices:3, 5\ncondition_number:300\nconstrained_solve:off\n\
             gain_correction:False\ngain_correction_rate:1\nchannel_check_db:none\n\
             drive_rms_limit:1.0,2.0\nuse_buzz:1\nsomething_unknown:ignored",
        )
        .unwrap();
        assert_eq!(p.df, 2.5);
        assert_eq!(p.oct_order, 3);
        assert_eq!(p.kappa_rule, KappaRule::Knee);
        assert_eq!(p.regularization, Regularization::None);
        assert_eq!(p.drive_condition_number, 50.0);
        assert!(p.hold_capped_bands);
        assert_eq!(p.min_coherence, Some(0.8));
        assert_eq!(p.tf_update_blend, TfUpdateBlend::Beta(0.2));
        assert_eq!(p.n_e_seed, Some(10.0));
        assert_eq!(p.limit_channel_indices, vec![3, 5]);
        assert_eq!(p.condition_number, Some(300.0));
        assert!(!p.constrained_solve);
        assert!(!p.gain_correction);
        assert_eq!(p.channel_check_db, None);
        assert_eq!(p.drive_rms_limit, Some(vec![1.0, 2.0]));
        assert!(p.use_buzz && p.control_to_cross_terms);
    }

    #[test]
    fn bad_values_are_refused() {
        assert!(Params::parse("oct_order:6").is_err());
        assert!(Params::parse("df:abc").is_err());
        assert!(Params::parse("df:1\nkappa_rule:best").is_err());
        assert!(Params::parse("df:1\nregularization:lots").is_err());
        assert!(Params::parse("df:1\nlimit_channel_indices:0").is_err());
    }
}
