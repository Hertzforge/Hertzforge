//! A checker for `schema/protocol.schema.json`: the JSON Schema 2020-12 keywords the
//! protocol schema uses, and nothing else. A keyword it does not implement fails the
//! check loudly rather than passing silently, so a schema that grows a keyword
//! cannot make these tests vacuous. (`pattern` is used once, on analysis operation
//! ids, which the control service never sends.)

use serde_json::Value;
use std::path::PathBuf;

pub struct Schema {
    root: Value,
}

const ANNOTATIONS: &[&str] = &["$schema", "$id", "$defs", "title", "description", "default"];

impl Schema {
    pub fn load() -> Schema {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schema/protocol.schema.json");
        let text = std::fs::read_to_string(&path).expect("the protocol schema exists");
        Schema {
            root: serde_json::from_str(&text).expect("the schema is JSON"),
        }
    }

    /// Check `instance` against the whole-message schema (`None`) or one definition.
    pub fn check(&self, instance: &Value, def: Option<&str>) -> Result<(), Vec<String>> {
        let schema = match def {
            None => self.root.clone(),
            Some(d) => self.root["$defs"][d].clone(),
        };
        assert!(!schema.is_null(), "no schema definition {def:?}");
        let mut errors = Vec::new();
        self.walk(&schema, instance, "$", &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Panic with the findings when `instance` does not validate.
    pub fn assert_valid(&self, instance: &Value, def: Option<&str>) {
        if let Err(e) = self.check(instance, def) {
            panic!(
                "{} does not validate: {}\n{}",
                def.unwrap_or("message"),
                e.join("; "),
                serde_json::to_string(instance).unwrap_or_default()
            );
        }
    }

    fn resolve<'a>(&'a self, r: &str) -> &'a Value {
        let name = r
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("unsupported $ref {r}"));
        let v = &self.root["$defs"][name];
        assert!(!v.is_null(), "no definition {name}");
        v
    }

    fn ok(&self, schema: &Value, instance: &Value) -> bool {
        let mut e = Vec::new();
        self.walk(schema, instance, "$", &mut e);
        e.is_empty()
    }

    fn walk(&self, schema: &Value, inst: &Value, path: &str, errs: &mut Vec<String>) {
        let s = match schema {
            Value::Bool(true) => return,
            Value::Bool(false) => {
                errs.push(format!("{path}: no value is allowed here"));
                return;
            }
            Value::Object(s) => s,
            other => panic!("schema at {path} is not an object: {other}"),
        };
        for (k, v) in s {
            match k.as_str() {
                "$ref" => self.walk(self.resolve(v.as_str().unwrap()), inst, path, errs),
                "type" => {
                    let types: Vec<&str> = match v {
                        Value::String(t) => vec![t.as_str()],
                        Value::Array(a) => a.iter().map(|t| t.as_str().unwrap()).collect(),
                        _ => panic!("bad type at {path}"),
                    };
                    if !types.iter().any(|t| is_type(inst, t)) {
                        errs.push(format!("{path}: {inst} is not {types:?}"));
                    }
                }
                "enum" => {
                    if !v.as_array().unwrap().iter().any(|e| same(e, inst)) {
                        errs.push(format!("{path}: {inst} is not one of {v}"));
                    }
                }
                "const" => {
                    if !same(v, inst) {
                        errs.push(format!("{path}: {inst} is not {v}"));
                    }
                }
                "properties" => {
                    if let Value::Object(o) = inst {
                        for (name, sub) in v.as_object().unwrap() {
                            if let Some(x) = o.get(name) {
                                self.walk(sub, x, &format!("{path}.{name}"), errs);
                            }
                        }
                    }
                }
                "required" => {
                    if let Value::Object(o) = inst {
                        for name in v.as_array().unwrap() {
                            let name = name.as_str().unwrap();
                            if !o.contains_key(name) {
                                errs.push(format!("{path}: missing {name}"));
                            }
                        }
                    }
                }
                "additionalProperties" => {
                    if let Value::Object(o) = inst {
                        let known = s.get("properties").and_then(Value::as_object);
                        for (name, x) in o {
                            if known.is_some_and(|k| k.contains_key(name)) {
                                continue;
                            }
                            self.walk(v, x, &format!("{path}.{name}"), errs);
                        }
                    }
                }
                "items" => {
                    if let Value::Array(a) = inst {
                        let skip = s
                            .get("prefixItems")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len);
                        for (i, x) in a.iter().enumerate().skip(skip) {
                            self.walk(v, x, &format!("{path}[{i}]"), errs);
                        }
                    }
                }
                "prefixItems" => {
                    if let Value::Array(a) = inst {
                        for (i, (sub, x)) in v.as_array().unwrap().iter().zip(a).enumerate() {
                            self.walk(sub, x, &format!("{path}[{i}]"), errs);
                        }
                    }
                }
                "minItems" => {
                    if let Value::Array(a) = inst {
                        if (a.len() as u64) < v.as_u64().unwrap() {
                            errs.push(format!("{path}: fewer than {v} items"));
                        }
                    }
                }
                "maxItems" => {
                    if let Value::Array(a) = inst {
                        if (a.len() as u64) > v.as_u64().unwrap() {
                            errs.push(format!("{path}: more than {v} items"));
                        }
                    }
                }
                "maxLength" => {
                    if let Value::String(t) = inst {
                        if (t.chars().count() as u64) > v.as_u64().unwrap() {
                            errs.push(format!("{path}: longer than {v}"));
                        }
                    }
                }
                "maximum" => {
                    if let Some(x) = inst.as_f64() {
                        if x > v.as_f64().unwrap() {
                            errs.push(format!("{path}: {x} above {v}"));
                        }
                    }
                }
                "minLength" => {
                    if let Value::String(t) = inst {
                        if (t.chars().count() as u64) < v.as_u64().unwrap() {
                            errs.push(format!("{path}: shorter than {v}"));
                        }
                    }
                }
                "minimum" => {
                    if let Some(x) = inst.as_f64() {
                        if x < v.as_f64().unwrap() {
                            errs.push(format!("{path}: {x} below {v}"));
                        }
                    }
                }
                "exclusiveMinimum" => {
                    if let Some(x) = inst.as_f64() {
                        if x <= v.as_f64().unwrap() {
                            errs.push(format!("{path}: {x} not above {v}"));
                        }
                    }
                }
                "multipleOf" => {
                    if let Some(x) = inst.as_f64() {
                        let m = v.as_f64().unwrap();
                        if (x / m).fract() != 0.0 {
                            errs.push(format!("{path}: {x} not a multiple of {m}"));
                        }
                    }
                }
                "oneOf" => {
                    let n = v
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|sub| self.ok(sub, inst))
                        .count();
                    if n != 1 {
                        errs.push(format!("{path}: {n} of oneOf match, not exactly 1"));
                    }
                }
                "allOf" => {
                    for sub in v.as_array().unwrap() {
                        self.walk(sub, inst, path, errs);
                    }
                }
                "if" => {
                    let branch = if self.ok(v, inst) { "then" } else { "else" };
                    if let Some(sub) = s.get(branch) {
                        self.walk(sub, inst, path, errs);
                    }
                }
                "then" | "else" => {}
                k if ANNOTATIONS.contains(&k) => {}
                other => panic!("the schema checker does not implement \"{other}\" (at {path})"),
            }
        }
    }
}

fn is_type(v: &Value, t: &str) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "number" => v.is_number(),
        "integer" => v.as_f64().is_some_and(|x| x.fract() == 0.0),
        other => panic!("unknown type {other}"),
    }
}

/// JSON equality with numbers compared by value.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}
