//! Builders for the viz kit's `spectrum` and `reading` views (schema/PROTOCOL.md,
//! "Views"). Keys are written in contract order; series data is attached to an
//! [`Arrays`] set as `f64` vectors and referenced from the view.

use crate::codec::Arrays;
use serde_json::{Map, Value};

/// Axis scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Log,
    Linear,
}

impl Scale {
    pub fn as_str(self) -> &'static str {
        match self {
            Scale::Log => "log",
            Scale::Linear => "linear",
        }
    }
}

/// A plot axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub label: String,
    pub unit: Option<String>,
    pub scale: Scale,
}

impl Axis {
    pub fn new(label: &str, unit: Option<&str>, scale: Scale) -> Axis {
        Axis {
            label: label.to_owned(),
            unit: unit.map(str::to_owned),
            scale,
        }
    }

    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("label".into(), Value::from(self.label.as_str()));
        m.insert(
            "unit".into(),
            self.unit.as_deref().map_or(Value::Null, Value::from),
        );
        m.insert("scale".into(), Value::from(self.scale.as_str()));
        Value::Object(m)
    }
}

/// A series role; it sets the line style so no meaning rests on color alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Measured,
    Reference,
    Tolerance,
    Abort,
    Drive,
    Other,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Measured => "measured",
            Role::Reference => "reference",
            Role::Tolerance => "tolerance",
            Role::Abort => "abort",
            Role::Drive => "drive",
            Role::Other => "other",
        }
    }
}

/// One x-y curve. `edges` set means the values are binned (`shape: "steps"`).
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub name: String,
    pub role: Role,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub edges: Option<Vec<f64>>,
}

impl Series {
    pub fn new(name: &str, role: Role, x: Vec<f64>, y: Vec<f64>) -> Series {
        Series {
            name: name.to_owned(),
            role,
            x,
            y,
            edges: None,
        }
    }

    /// Mark the values as a density over each line's own bin, drawn flat across it
    /// (schema/PROTOCOL.md, `shape: "steps"`), with the bins from [`bin_edges`].
    pub fn stepped(mut self) -> Series {
        self.edges = Some(bin_edges(&self.x));
        self
    }
}

/// The edges of the bins a set of evenly spaced line centers represents: midway
/// between lines, and the outer edges as far beyond the end lines as the nearest
/// midpoints are inside them, so every bin of an FFT spectrum is `df` wide. One more
/// edge than there are lines. The same rule the analysis side draws with.
pub fn bin_edges(centers: &[f64]) -> Vec<f64> {
    let n = centers.len();
    if n < 2 {
        return centers.to_vec();
    }
    let mut edges = Vec::with_capacity(n + 1);
    edges.push(centers[0] - 0.5 * (centers[1] - centers[0]));
    edges.extend(centers.windows(2).map(|w| 0.5 * (w[0] + w[1])));
    edges.push(centers[n - 1] + 0.5 * (centers[n - 1] - centers[n - 2]));
    edges
}

/// A `spectrum` view. Each series' `x` then `y` is attached to `arrays` in series
/// order (every array is referenced exactly once, so each series carries its own x).
pub fn spectrum(
    arrays: &mut Arrays,
    title: &str,
    x: &Axis,
    y: &Axis,
    series: Vec<Series>,
    link: Option<&str>,
) -> Value {
    let mut m = Map::new();
    m.insert("view".into(), Value::from("spectrum"));
    m.insert("title".into(), Value::from(title));
    m.insert("x".into(), x.to_json());
    m.insert("y".into(), y.to_json());
    let series: Vec<Value> = series
        .into_iter()
        .map(|s| {
            let mut e = Map::new();
            e.insert("name".into(), Value::from(s.name));
            e.insert("role".into(), Value::from(s.role.as_str()));
            e.insert("x".into(), arrays.push_f64(s.x));
            e.insert("y".into(), arrays.push_f64(s.y));
            if let Some(edges) = s.edges {
                e.insert("shape".into(), Value::from("steps"));
                e.insert("edges".into(), arrays.push_f64(edges));
            }
            Value::Object(e)
        })
        .collect();
    m.insert("series".into(), Value::Array(series));
    if let Some(link) = link {
        m.insert("link".into(), Value::from(link));
    }
    Value::Object(m)
}

/// A reading row's status, shown as a word or symbol as well as a color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Info,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Info => "info",
        }
    }
}

/// A reading row's value.
#[derive(Debug, Clone, PartialEq)]
pub enum RowValue {
    Number(f64),
    Text(String),
}

/// One reading row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub value: RowValue,
    pub unit: Option<String>,
    pub status: Status,
}

impl Row {
    pub fn number(label: &str, value: f64, unit: Option<&str>, status: Status) -> Row {
        Row {
            label: label.to_owned(),
            value: RowValue::Number(value),
            unit: unit.map(str::to_owned),
            status,
        }
    }

    pub fn text(label: &str, value: &str, status: Status) -> Row {
        Row {
            label: label.to_owned(),
            value: RowValue::Text(value.to_owned()),
            unit: None,
            status,
        }
    }
}

/// A `reading` view.
pub fn reading(title: &str, rows: Vec<Row>) -> Value {
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let mut e = Map::new();
            e.insert("label".into(), Value::from(r.label));
            e.insert(
                "value".into(),
                match r.value {
                    RowValue::Number(x) => {
                        serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
                    }
                    RowValue::Text(s) => Value::from(s),
                },
            );
            e.insert("unit".into(), r.unit.map_or(Value::Null, Value::from));
            e.insert("status".into(), Value::from(r.status.as_str()));
            Value::Object(e)
        })
        .collect();
    let mut m = Map::new();
    m.insert("view".into(), Value::from("reading"));
    m.insert("title".into(), Value::from(title));
    m.insert("rows".into(), Value::Array(rows));
    Value::Object(m)
}

/// Round to `digits` decimal places, for readings (keeps frames short and the
/// displayed value free of float noise).
pub fn round_to(x: f64, digits: i32) -> f64 {
    let p = 10f64.powi(digits);
    (x * p).round() / p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Arrays;

    #[test]
    fn bins_are_df_wide_and_centered_on_their_lines() {
        let x: Vec<f64> = (1..=5).map(|k| 2.5 * k as f64).collect();
        let e = bin_edges(&x);
        assert_eq!(e.len(), x.len() + 1);
        for w in e.windows(2) {
            assert!((w[1] - w[0] - 2.5).abs() < 1e-12);
        }
        for (k, &c) in x.iter().enumerate() {
            assert!((0.5 * (e[k] + e[k + 1]) - c).abs() < 1e-12);
        }
    }

    #[test]
    fn a_stepped_series_carries_its_shape_and_edges() {
        let mut arrays = Arrays::default();
        let s = Series::new(
            "Control",
            Role::Measured,
            vec![10.0, 20.0, 30.0],
            vec![1.0, 2.0, 3.0],
        )
        .stepped();
        let axis = Axis::new("Frequency", Some("Hz"), Scale::Log);
        let v = spectrum(&mut arrays, "t", &axis, &axis, vec![s], None);
        let series = &v["series"][0];
        assert_eq!(series["shape"], "steps");
        assert!(series["edges"]["$array"].is_string());
        let keys: Vec<&str> = series
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, ["name", "role", "x", "y", "shape", "edges"]);
    }
}
