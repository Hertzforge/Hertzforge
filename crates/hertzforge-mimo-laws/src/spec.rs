//! What a built-in law declares about itself, as plain data: the one
//! declaration both its law file's manifest (through the SDK) and the
//! controller's catalog entry (through the law host's types) are built from.

use hertzforge_sdm::law::{LawError, MimoInput, MimoShape};

/// The kind, limits and default of one parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamKindDecl {
    /// A real number.
    Number {
        /// Unit shown beside the box.
        unit: &'static str,
        /// Smallest accepted value.
        min: f64,
        /// Largest accepted value.
        max: f64,
        /// A hint for the box's step.
        step: f64,
        /// The default.
        default: f64,
    },
    /// A whole number.
    Integer {
        /// Unit shown beside the box.
        unit: &'static str,
        /// Smallest accepted value.
        min: i64,
        /// Largest accepted value.
        max: i64,
        /// A hint for the box's step.
        step: i64,
        /// The default.
        default: i64,
    },
    /// On or off, with its default.
    Toggle(bool),
    /// One of a set of named options.
    Choice {
        /// The options, in form order.
        options: &'static [&'static str],
        /// One of `options`.
        default: &'static str,
    },
}

/// One declared parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDecl {
    /// Stable key, `[a-z][a-z0-9_]*`.
    pub key: &'static str,
    /// Name on the form.
    pub label: &'static str,
    /// Help text.
    pub help: &'static str,
    /// Kind, limits and default.
    pub kind: ParamKindDecl,
}

/// The layout of a diagnostic's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagKind {
    /// One number per loop.
    Scalar,
    /// One number per line.
    PerLine,
    /// One number per control per line, line-major.
    PerControlLine,
    /// One number per drive per line, line-major.
    PerDriveLine,
}

/// One declared diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagDecl {
    /// Stable key.
    pub key: &'static str,
    /// Name on a plot.
    pub label: &'static str,
    /// Unit.
    pub unit: &'static str,
    /// Help text.
    pub help: &'static str,
    /// Layout.
    pub kind: DiagKind,
}

/// Everything a built-in law declares.
#[derive(Debug, Clone, PartialEq)]
pub struct LawSpec {
    /// Short name.
    pub name: &'static str,
    /// The law's own version.
    pub version: &'static str,
    /// Author.
    pub author: &'static str,
    /// One paragraph on what it does.
    pub summary: &'static str,
    /// Parameters, in form order.
    pub params: Vec<ParamDecl>,
    /// Diagnostics.
    pub diagnostics: Vec<DiagDecl>,
    /// Supported shapes.
    pub shapes: Vec<MimoShape>,
    /// Optional inputs read.
    pub needs: Vec<MimoInput>,
}

/// A parameter value as a built-in law reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamVal {
    /// A number.
    Number(f64),
    /// An integer.
    Integer(i64),
    /// A toggle.
    Toggle(bool),
    /// A choice, by option name.
    Choice(String),
}

impl ParamVal {
    /// The value as a number (an integer widens).
    pub fn number(&self, key: &str) -> Result<f64, LawError> {
        match self {
            ParamVal::Number(v) => Ok(*v),
            ParamVal::Integer(v) => Ok(*v as f64),
            _ => Err(LawError(format!("parameter `{key}` is not a number"))),
        }
    }

    /// The value as an integer.
    pub fn integer(&self, key: &str) -> Result<i64, LawError> {
        match self {
            ParamVal::Integer(v) => Ok(*v),
            _ => Err(LawError(format!("parameter `{key}` is not an integer"))),
        }
    }

    /// The value as a toggle.
    pub fn toggle(&self, key: &str) -> Result<bool, LawError> {
        match self {
            ParamVal::Toggle(v) => Ok(*v),
            _ => Err(LawError(format!("parameter `{key}` is not a toggle"))),
        }
    }

    /// The value as a choice.
    pub fn choice(&self, key: &str) -> Result<&str, LawError> {
        match self {
            ParamVal::Choice(v) => Ok(v),
            _ => Err(LawError(format!("parameter `{key}` is not a choice"))),
        }
    }
}

impl ParamDecl {
    /// The declared default.
    pub fn default_value(&self) -> ParamVal {
        match &self.kind {
            ParamKindDecl::Number { default, .. } => ParamVal::Number(*default),
            ParamKindDecl::Integer { default, .. } => ParamVal::Integer(*default),
            ParamKindDecl::Toggle(d) => ParamVal::Toggle(*d),
            ParamKindDecl::Choice { default, .. } => ParamVal::Choice((*default).to_string()),
        }
    }

    /// Whether `v` is of this parameter's kind and within its limits.
    pub fn check(&self, v: &ParamVal) -> Result<(), LawError> {
        let fail = |why: String| Err(LawError(format!("parameter `{}`: {why}", self.key)));
        match (&self.kind, v) {
            (ParamKindDecl::Number { min, max, .. }, ParamVal::Number(x)) => {
                if !(x.is_finite() && x >= min && x <= max) {
                    return fail(format!("{x} is outside [{min}, {max}]"));
                }
            }
            (ParamKindDecl::Integer { min, max, .. }, ParamVal::Integer(x)) => {
                if x < min || x > max {
                    return fail(format!("{x} is outside [{min}, {max}]"));
                }
            }
            (ParamKindDecl::Toggle(_), ParamVal::Toggle(_)) => {}
            (ParamKindDecl::Choice { options, .. }, ParamVal::Choice(x)) => {
                if !options.contains(&x.as_str()) {
                    return fail(format!("`{x}` is not one of {}", options.join(", ")));
                }
            }
            _ => return fail("the value is not of the declared kind".into()),
        }
        Ok(())
    }
}

impl LawSpec {
    /// Every declared parameter with `given` values in place of the defaults,
    /// each checked against its declaration: what a law is built from. A key
    /// that is not declared is refused.
    pub fn resolve(&self, given: &[(&str, ParamVal)]) -> Result<Vec<(String, ParamVal)>, LawError> {
        for (k, _) in given {
            if !self.params.iter().any(|p| p.key == *k) {
                return Err(LawError(format!("parameter `{k}` is not declared")));
            }
        }
        self.params
            .iter()
            .map(|p| {
                let v = given
                    .iter()
                    .find(|(k, _)| *k == p.key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| p.default_value());
                p.check(&v)?;
                Ok((p.key.to_string(), v))
            })
            .collect()
    }
}
