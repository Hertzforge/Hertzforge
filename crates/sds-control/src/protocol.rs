//! `sds/1` message envelopes (schema/PROTOCOL.md, "Messages").
//!
//! Every constructor writes keys in the order the contract lists them, so a header
//! encodes the same bytes in every language.

use serde_json::{json, Map, Value};

/// The protocol version string the client must match exactly.
pub const PROTOCOL: &str = "sds/1";
/// This service's name in `hello`.
pub const SERVICE: &str = "control";
/// This service's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The request id used when a request is too malformed to carry a readable one.
/// The contract gives no rule for this case; `-1` cannot collide with a client id
/// that follows the usual counting-up convention.
pub const UNREADABLE_ID: i64 = -1;

/// Error codes, as listed in the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest,
    UnknownMethod,
    NotFound,
    Refused,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::UnknownMethod => "unknown_method",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Refused => "refused",
            ErrorCode::Internal => "internal",
        }
    }
}

/// An error a method returns: the code plus a message a user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodError {
    pub code: ErrorCode,
    pub message: String,
}

impl MethodError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> MethodError {
        MethodError {
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> MethodError {
        MethodError::new(ErrorCode::BadRequest, message)
    }

    pub fn refused(message: impl Into<String>) -> MethodError {
        MethodError::new(ErrorCode::Refused, message)
    }

    pub fn not_found(message: impl Into<String>) -> MethodError {
        MethodError::new(ErrorCode::NotFound, message)
    }

    pub fn internal(message: impl Into<String>) -> MethodError {
        MethodError::new(ErrorCode::Internal, message)
    }
}

/// The greeting the server sends first on every connection.
pub fn hello() -> Value {
    json!({"type": "hello", "protocol": PROTOCOL, "service": SERVICE, "version": VERSION})
}

/// A success response.
pub fn response(id: i64, result: Value) -> Value {
    json!({"type": "response", "id": id, "result": result})
}

/// An error response.
pub fn error_response(id: i64, error: &MethodError) -> Value {
    json!({"type": "response", "id": id,
           "error": {"code": error.code.as_str(), "message": error.message}})
}

/// An event.
pub fn event(topic: &str, seq: u64, data: Value) -> Value {
    json!({"type": "event", "topic": topic, "seq": seq, "data": data})
}

/// A parsed request.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: i64,
    pub method: String,
    pub params: Map<String, Value>,
}

/// Parse a request message. On failure returns the id to answer with (the
/// request's own when readable) and the error.
pub fn parse_request(message: &Value) -> Result<Request, (i64, MethodError)> {
    let id = message.get("id").and_then(Value::as_i64);
    let reply_id = id.unwrap_or(UNREADABLE_ID);
    let fail = |text: &str| (reply_id, MethodError::bad_request(text));
    match message.get("type").and_then(Value::as_str) {
        Some("request") => {}
        Some(other) => {
            return Err(fail(&format!(
                "this service accepts only \"request\" messages from a client, not \"{other}\""
            )))
        }
        None => {
            return Err(fail(
                "the message has no \"type\"; send {\"type\":\"request\",...}",
            ))
        }
    }
    let Some(id) = id else {
        return Err(fail("the request has no integer \"id\""));
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Err(fail("the request has no \"method\" string"));
    };
    let params = match message.get("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(fail("\"params\" must be an object")),
    };
    Ok(Request {
        id,
        method: method.to_owned(),
        params,
    })
}

/// Read an optional number parameter.
pub fn param_f64(params: &Map<String, Value>, name: &str) -> Result<Option<f64>, MethodError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite())
            .map(Some)
            .ok_or_else(|| MethodError::bad_request(format!("\"{name}\" must be a finite number"))),
    }
}

/// Read an optional string parameter.
pub fn param_str<'a>(
    params: &'a Map<String, Value>,
    name: &str,
) -> Result<Option<&'a str>, MethodError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(MethodError::bad_request(format!(
            "\"{name}\" must be a string"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_names_the_control_service() {
        assert_eq!(
            serde_json::to_string(&hello()).unwrap(),
            format!(
                r#"{{"type":"hello","protocol":"sds/1","service":"control","version":"{VERSION}"}}"#
            )
        );
    }

    #[test]
    fn error_response_matches_the_golden_layout() {
        // schema/golden/response_error.json, re-serialized compactly.
        let e = MethodError::not_found(
            "No dataset named \"plate-x\"; catalog.list names the ones that exist.",
        );
        let golden = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../schema/golden/response_error.json"
        ))
        .unwrap();
        let golden: Value = serde_json::from_str(&golden).unwrap();
        assert_eq!(
            serde_json::to_string(&error_response(8, &e)).unwrap(),
            serde_json::to_string(&golden).unwrap()
        );
    }

    #[test]
    fn malformed_requests_get_bad_request_with_the_best_id() {
        let (id, e) = parse_request(&json!({"type": "request", "id": 3})).unwrap_err();
        assert_eq!((id, e.code), (3, ErrorCode::BadRequest));
        let (id, _) = parse_request(&json!({"method": "x"})).unwrap_err();
        assert_eq!(id, UNREADABLE_ID);
        let ok =
            parse_request(&json!({"type": "request", "id": 4, "method": "sim.state"})).unwrap();
        assert!(ok.params.is_empty());
    }
}
