//! The `sds/1` frame codec (schema/PROTOCOL.md, "Frames").
//!
//! A message that carries no arrays travels as a text frame: one compact UTF-8 JSON
//! object. A message that carries arrays travels as a binary frame:
//!
//! ```text
//! "SDS1" | u32 LE header length H | header JSON, space padded so 8 + H % 8 == 0 | payload
//! ```
//!
//! The header is the message plus a trailing `"arrays"` list; inside the message each
//! array is the reference object `{"$array": "<id>"}`. Each array starts at a multiple
//! of 8 bytes from the payload start, little-endian, C order.
//!
//! Byte-for-byte agreement with the Python reference writer (`schema/make_golden.py`)
//! rests on three things: compact separators, UTF-8 left unescaped (serde_json's
//! default), and key order preserved as written (`serde_json/preserve_order`).
//! The golden frames in `schema/golden/` pin all three (tests/golden.rs).
//!
//! Decoding keeps the `{"$array": id}` references in the message and returns the
//! typed arrays beside it in [`Decoded::arrays`]; [`Decoded::expanded`] produces the
//! inline `{"dtype","shape","values"}` form the golden `*.expected.json` files use.

use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// The four-byte frame magic.
pub const MAGIC: &[u8; 4] = b"SDS1";

/// Key that marks an array reference inside a message.
pub const ARRAY_REF_KEY: &str = "$array";

/// A codec failure, with a message that names what was wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

fn err<T>(msg: impl Into<String>) -> Result<T, CodecError> {
    Err(CodecError(msg.into()))
}

/// Element type of an array on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    F32,
    F64,
    I32,
    U32,
    U8,
}

impl DType {
    /// The protocol name (`"f32"` ...).
    pub fn name(self) -> &'static str {
        match self {
            DType::F32 => "f32",
            DType::F64 => "f64",
            DType::I32 => "i32",
            DType::U32 => "u32",
            DType::U8 => "u8",
        }
    }

    /// Bytes per element.
    pub fn size(self) -> usize {
        match self {
            DType::F64 => 8,
            DType::F32 | DType::I32 | DType::U32 => 4,
            DType::U8 => 1,
        }
    }

    /// Parse a protocol dtype name.
    pub fn parse(name: &str) -> Option<DType> {
        match name {
            "f32" => Some(DType::F32),
            "f64" => Some(DType::F64),
            "i32" => Some(DType::I32),
            "u32" => Some(DType::U32),
            "u8" => Some(DType::U8),
            _ => None,
        }
    }
}

/// Typed array storage.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayData {
    F32(Vec<f32>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    U32(Vec<u32>),
    U8(Vec<u8>),
}

impl ArrayData {
    pub fn dtype(&self) -> DType {
        match self {
            ArrayData::F32(_) => DType::F32,
            ArrayData::F64(_) => DType::F64,
            ArrayData::I32(_) => DType::I32,
            ArrayData::U32(_) => DType::U32,
            ArrayData::U8(_) => DType::U8,
        }
    }

    /// Element count.
    pub fn len(&self) -> usize {
        match self {
            ArrayData::F32(v) => v.len(),
            ArrayData::F64(v) => v.len(),
            ArrayData::I32(v) => v.len(),
            ArrayData::U32(v) => v.len(),
            ArrayData::U8(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn write_le(&self, out: &mut Vec<u8>) {
        match self {
            ArrayData::F32(v) => v
                .iter()
                .for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
            ArrayData::F64(v) => v
                .iter()
                .for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
            ArrayData::I32(v) => v
                .iter()
                .for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
            ArrayData::U32(v) => v
                .iter()
                .for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
            ArrayData::U8(v) => out.extend_from_slice(v),
        }
    }

    fn read_le(dtype: DType, bytes: &[u8]) -> ArrayData {
        fn chunks<const N: usize>(bytes: &[u8]) -> impl Iterator<Item = [u8; N]> + '_ {
            bytes.chunks_exact(N).map(|c| {
                let mut a = [0u8; N];
                a.copy_from_slice(c);
                a
            })
        }
        match dtype {
            DType::F32 => ArrayData::F32(chunks::<4>(bytes).map(f32::from_le_bytes).collect()),
            DType::F64 => ArrayData::F64(chunks::<8>(bytes).map(f64::from_le_bytes).collect()),
            DType::I32 => ArrayData::I32(chunks::<4>(bytes).map(i32::from_le_bytes).collect()),
            DType::U32 => ArrayData::U32(chunks::<4>(bytes).map(u32::from_le_bytes).collect()),
            DType::U8 => ArrayData::U8(bytes.to_vec()),
        }
    }

    /// The values as JSON numbers (non-finite floats become `null`).
    pub fn to_json_values(&self) -> Vec<Value> {
        fn float(x: f64) -> Value {
            serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
        }
        match self {
            ArrayData::F32(v) => v.iter().map(|&x| float(f64::from(x))).collect(),
            ArrayData::F64(v) => v.iter().map(|&x| float(x)).collect(),
            ArrayData::I32(v) => v.iter().map(|&x| Value::from(x)).collect(),
            ArrayData::U32(v) => v.iter().map(|&x| Value::from(x)).collect(),
            ArrayData::U8(v) => v.iter().map(|&x| Value::from(x)).collect(),
        }
    }
}

/// An n-dimensional array: data in C order plus its shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Array {
    pub shape: Vec<usize>,
    pub data: ArrayData,
}

impl Array {
    /// An array of `shape`; fails when the shape does not hold exactly `data.len()`
    /// elements.
    pub fn new(shape: Vec<usize>, data: ArrayData) -> Result<Array, CodecError> {
        let count: usize = shape.iter().product();
        if count != data.len() {
            return err(format!(
                "array shape {shape:?} holds {count} elements but {} were given",
                data.len()
            ));
        }
        Ok(Array { shape, data })
    }

    /// A one-dimensional array.
    pub fn vector(data: ArrayData) -> Array {
        Array {
            shape: vec![data.len()],
            data,
        }
    }

    pub fn dtype(&self) -> DType {
        self.data.dtype()
    }

    /// The values widened to `f64` (every protocol dtype converts exactly).
    pub fn to_f64(&self) -> Vec<f64> {
        match &self.data {
            ArrayData::F32(v) => v.iter().map(|&x| f64::from(x)).collect(),
            ArrayData::F64(v) => v.clone(),
            ArrayData::I32(v) => v.iter().map(|&x| f64::from(x)).collect(),
            ArrayData::U32(v) => v.iter().map(|&x| f64::from(x)).collect(),
            ArrayData::U8(v) => v.iter().map(|&x| f64::from(x)).collect(),
        }
    }
}

/// The arrays attached to an outgoing message, in payload order.
#[derive(Debug, Clone, Default)]
pub struct Arrays {
    entries: Vec<(String, Array)>,
}

impl Arrays {
    pub fn new() -> Arrays {
        Arrays::default()
    }

    /// Attach `array` under `id` and return the reference object to place in the
    /// message.
    pub fn insert(&mut self, id: impl Into<String>, array: Array) -> Value {
        let id = id.into();
        let reference = array_ref(&id);
        self.entries.push((id, array));
        reference
    }

    /// Attach `array` under the next automatic id (`a0`, `a1`, ...).
    pub fn push(&mut self, array: Array) -> Value {
        let id = format!("a{}", self.entries.len());
        self.insert(id, array)
    }

    /// Attach a one-dimensional `f64` array under the next automatic id.
    pub fn push_f64(&mut self, values: Vec<f64>) -> Value {
        self.push(Array::vector(ArrayData::F64(values)))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Array)> {
        self.entries.iter().map(|(id, a)| (id.as_str(), a))
    }
}

/// The reference object `{"$array": id}`.
pub fn array_ref(id: &str) -> Value {
    let mut m = Map::new();
    m.insert(ARRAY_REF_KEY.to_owned(), Value::String(id.to_owned()));
    Value::Object(m)
}

/// The id of `value` if it is an array reference (an object whose only key is
/// `$array`, holding a string).
pub fn as_array_ref(value: &Value) -> Option<&str> {
    match value {
        Value::Object(m) if m.len() == 1 => m.get(ARRAY_REF_KEY).and_then(Value::as_str),
        _ => None,
    }
}

/// Count every array reference in `value` by id. Fails on a malformed reference
/// (an object carrying `$array` beside other keys, or a non-string id).
fn count_refs(value: &Value, counts: &mut HashMap<String, usize>) -> Result<(), CodecError> {
    match value {
        Value::Object(m) => {
            if let Some(id) = as_array_ref(value) {
                *counts.entry(id.to_owned()).or_insert(0) += 1;
            } else if m.contains_key(ARRAY_REF_KEY) {
                return err("an array reference must be exactly {\"$array\": \"<id>\"}");
            } else {
                for v in m.values() {
                    count_refs(v, counts)?;
                }
            }
        }
        Value::Array(items) => {
            for v in items {
                count_refs(v, counts)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Check that every listed id is referenced exactly once and every reference names a
/// listed id.
fn check_refs<'a>(message: &Value, ids: impl Iterator<Item = &'a str>) -> Result<(), CodecError> {
    let mut counts = HashMap::new();
    count_refs(message, &mut counts)?;
    let mut listed = 0usize;
    for id in ids {
        listed += 1;
        match counts.get(id) {
            Some(1) => {}
            Some(n) => {
                return err(format!(
                    "array \"{id}\" is referenced {n} times; once is required"
                ))
            }
            None => return err(format!("array \"{id}\" is listed but never referenced")),
        }
    }
    if counts.len() != listed {
        let mut missing: Vec<&String> = counts.keys().collect();
        missing.sort();
        return err(format!(
            "the message references arrays that are not attached: {missing:?}"
        ));
    }
    Ok(())
}

/// One outgoing WebSocket frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}

/// Frame `message`: a text frame when it carries no arrays, a binary frame otherwise.
pub fn encode_frame(message: &Value, arrays: &Arrays) -> Result<Frame, CodecError> {
    if arrays.is_empty() {
        check_refs(message, std::iter::empty())?;
        let text = serde_json::to_string(message)
            .map_err(|e| CodecError(format!("message does not serialize: {e}")))?;
        Ok(Frame::Text(text))
    } else {
        encode(message, arrays).map(Frame::Binary)
    }
}

/// Encode a binary frame. `message` must be an object without an `arrays` key, and
/// must reference each attached array exactly once.
pub fn encode(message: &Value, arrays: &Arrays) -> Result<Vec<u8>, CodecError> {
    let Value::Object(fields) = message else {
        return err("a framed message must be a JSON object");
    };
    if fields.contains_key("arrays") {
        return err("the message already has an \"arrays\" key; the codec writes that list");
    }
    check_refs(message, arrays.iter().map(|(id, _)| id))?;

    let mut payload: Vec<u8> = Vec::new();
    let mut entries: Vec<Value> = Vec::with_capacity(arrays.len());
    for (id, array) in arrays.iter() {
        while !payload.len().is_multiple_of(8) {
            payload.push(0);
        }
        let offset = payload.len();
        array.data.write_le(&mut payload);
        let mut e = Map::new();
        e.insert("id".into(), Value::from(id));
        e.insert("dtype".into(), Value::from(array.dtype().name()));
        e.insert(
            "shape".into(),
            Value::Array(array.shape.iter().map(|&n| Value::from(n)).collect()),
        );
        e.insert("offset".into(), Value::from(offset));
        e.insert("nbytes".into(), Value::from(payload.len() - offset));
        entries.push(Value::Object(e));
    }

    let mut header_fields = fields.clone();
    header_fields.insert("arrays".into(), Value::Array(entries));
    let mut header = serde_json::to_vec(&Value::Object(header_fields))
        .map_err(|e| CodecError(format!("header does not serialize: {e}")))?;
    let pad = (8 - (8 + header.len()) % 8) % 8;
    header.resize(header.len() + pad, b' ');
    let header_len = u32::try_from(header.len())
        .map_err(|_| CodecError("header is larger than 4 GiB".into()))?;

    let mut frame = Vec::with_capacity(8 + header.len() + payload.len());
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&header_len.to_le_bytes());
    frame.extend_from_slice(&header);
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// A decoded message: the message with its `{"$array": id}` references intact (the
/// `arrays` list removed), and the typed arrays by id.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub message: Value,
    pub arrays: BTreeMap<String, Array>,
}

impl Decoded {
    /// The array a reference object points at.
    pub fn array(&self, reference: &Value) -> Option<&Array> {
        as_array_ref(reference).and_then(|id| self.arrays.get(id))
    }

    /// The message with every reference replaced by `{"dtype","shape","values"}`,
    /// the form the golden `*.expected.json` files are written in.
    pub fn expanded(&self) -> Value {
        fn walk(node: &Value, arrays: &BTreeMap<String, Array>) -> Value {
            if let Some(a) = as_array_ref(node).and_then(|id| arrays.get(id)) {
                let mut m = Map::new();
                m.insert("dtype".into(), Value::from(a.dtype().name()));
                m.insert(
                    "shape".into(),
                    Value::Array(a.shape.iter().map(|&n| Value::from(n)).collect()),
                );
                m.insert("values".into(), Value::Array(a.data.to_json_values()));
                return Value::Object(m);
            }
            match node {
                Value::Object(m) => Value::Object(
                    m.iter()
                        .map(|(k, v)| (k.clone(), walk(v, arrays)))
                        .collect(),
                ),
                Value::Array(items) => {
                    Value::Array(items.iter().map(|v| walk(v, arrays)).collect())
                }
                other => other.clone(),
            }
        }
        walk(&self.message, &self.arrays)
    }
}

/// Decode a text frame. Text frames carry no arrays, so any array reference is an
/// error.
pub fn decode_text(text: &str) -> Result<Value, CodecError> {
    let message: Value = serde_json::from_str(text)
        .map_err(|e| CodecError(format!("text frame is not JSON: {e}")))?;
    if !message.is_object() {
        return err("a message must be a JSON object");
    }
    check_refs(&message, std::iter::empty())?;
    Ok(message)
}

/// Decode a binary frame, validating the layout rules of PROTOCOL.md.
pub fn decode(frame: &[u8]) -> Result<Decoded, CodecError> {
    if frame.len() < 8 {
        return err(format!(
            "binary frame of {} bytes is shorter than its 8-byte prefix",
            frame.len()
        ));
    }
    if &frame[0..4] != MAGIC {
        return err("binary frame does not start with the magic \"SDS1\"");
    }
    let header_len = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
    let payload_start = 8usize
        .checked_add(header_len)
        .filter(|&end| end <= frame.len())
        .ok_or_else(|| {
            CodecError(format!(
                "header length {header_len} runs past the frame end"
            ))
        })?;
    if !payload_start.is_multiple_of(8) {
        return err(format!(
            "8 + header length ({payload_start}) is not a multiple of 8"
        ));
    }
    let header = std::str::from_utf8(&frame[8..payload_start])
        .map_err(|e| CodecError(format!("header is not UTF-8: {e}")))?;
    let mut message: Value = serde_json::from_str(header.trim_end_matches(' '))
        .map_err(|e| CodecError(format!("header is not JSON: {e}")))?;
    let Value::Object(fields) = &mut message else {
        return err("the header must be a JSON object");
    };
    let entries = match fields.shift_remove("arrays") {
        None => Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => return err("\"arrays\" must be a list"),
    };
    let payload = &frame[payload_start..];

    let mut arrays = BTreeMap::new();
    let mut order = Vec::with_capacity(entries.len());
    for entry in &entries {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| CodecError("an arrays entry has no string \"id\"".into()))?;
        let dtype_name = entry.get("dtype").and_then(Value::as_str).unwrap_or("");
        let dtype = DType::parse(dtype_name).ok_or_else(|| {
            CodecError(format!("array \"{id}\" has unknown dtype \"{dtype_name}\""))
        })?;
        let shape = entry
            .get("shape")
            .and_then(Value::as_array)
            .ok_or_else(|| CodecError(format!("array \"{id}\" has no shape list")))?
            .iter()
            .map(|n| n.as_u64().map(|n| n as usize))
            .collect::<Option<Vec<usize>>>()
            .ok_or_else(|| {
                CodecError(format!(
                    "array \"{id}\" shape must be non-negative integers"
                ))
            })?;
        let offset = entry
            .get("offset")
            .and_then(Value::as_u64)
            .ok_or_else(|| CodecError(format!("array \"{id}\" has no integer offset")))?
            as usize;
        let nbytes = entry
            .get("nbytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| CodecError(format!("array \"{id}\" has no integer nbytes")))?
            as usize;
        if !offset.is_multiple_of(8) {
            return err(format!(
                "array \"{id}\" offset {offset} is not a multiple of 8"
            ));
        }
        let count: usize = shape.iter().product();
        if count.checked_mul(dtype.size()) != Some(nbytes) {
            return err(format!(
                "array \"{id}\" nbytes {nbytes} does not equal {count} elements x {} bytes",
                dtype.size()
            ));
        }
        let end = offset
            .checked_add(nbytes)
            .filter(|&e| e <= payload.len())
            .ok_or_else(|| {
                CodecError(format!("array \"{id}\" runs past the end of the payload"))
            })?;
        let data = ArrayData::read_le(dtype, &payload[offset..end]);
        if arrays
            .insert(id.to_owned(), Array { shape, data })
            .is_some()
        {
            return err(format!("array id \"{id}\" is listed twice"));
        }
        order.push(id.to_owned());
    }
    check_refs(&message, order.iter().map(String::as_str))?;
    Ok(Decoded { message, arrays })
}

/// Re-encode a decoded message (its arrays in `order`), for round-trip checks.
pub fn reencode(decoded: &Decoded, order: &[&str]) -> Result<Vec<u8>, CodecError> {
    let mut arrays = Arrays::new();
    for id in order {
        let a = decoded
            .arrays
            .get(*id)
            .ok_or_else(|| CodecError(format!("no decoded array \"{id}\"")))?;
        arrays.insert(*id, a.clone());
    }
    encode(&decoded.message, &arrays)
}

/// The array ids of a binary frame in header (payload) order.
pub fn header_array_order(frame: &[u8]) -> Result<Vec<String>, CodecError> {
    if frame.len() < 8 || &frame[0..4] != MAGIC {
        return err("not an SDS1 binary frame");
    }
    let header_len = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
    let header = frame
        .get(8..8 + header_len)
        .ok_or_else(|| CodecError("header runs past the frame end".into()))?;
    let v: Value = serde_json::from_slice(header)
        .map_err(|e| CodecError(format!("header is not JSON: {e}")))?;
    Ok(v.get("arrays")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn header_is_padded_and_arrays_are_aligned() {
        let mut arrays = Arrays::new();
        let a = arrays.push(Array::vector(ArrayData::U8(vec![1, 2, 3])));
        let b = arrays.push_f64(vec![1.5]);
        let frame = encode(&json!({"type": "event", "a": a, "b": b}), &arrays).unwrap();
        let h = u32::from_le_bytes(frame[4..8].try_into().unwrap()) as usize;
        assert_eq!((8 + h) % 8, 0);
        let d = decode(&frame).unwrap();
        assert_eq!(d.arrays["a1"].to_f64(), vec![1.5]);
        assert_eq!(d.arrays["a0"].data, ArrayData::U8(vec![1, 2, 3]));
    }

    #[test]
    fn unreferenced_or_double_referenced_arrays_are_refused() {
        let mut arrays = Arrays::new();
        let r = arrays.push_f64(vec![1.0]);
        assert!(encode(&json!({"x": 1}), &arrays).is_err());
        assert!(encode(&json!({"x": r.clone(), "y": r}), &arrays).is_err());
        assert!(encode(&json!({"x": {"$array": "zz"}}), &Arrays::new()).is_err());
    }

    #[test]
    fn corrupt_frames_are_refused_with_a_reason() {
        assert!(decode(b"SDS").is_err());
        assert!(decode(b"XXXX\0\0\0\0").is_err());
        let mut arrays = Arrays::new();
        let r = arrays.push_f64(vec![1.0, 2.0]);
        let frame = encode(&json!({"v": r}), &arrays).unwrap();
        assert!(
            decode(&frame[..frame.len() - 1]).is_err(),
            "truncated payload"
        );
    }

    #[test]
    fn shape_must_match_the_element_count() {
        assert!(Array::new(vec![2, 2], ArrayData::F32(vec![0.0; 3])).is_err());
        assert!(Array::new(vec![2, 2], ArrayData::F32(vec![0.0; 4])).is_ok());
    }
}
