//! The JSON-RPC 2.0 message model as LSP uses it (LSP 3.17 §messages):
//! requests, notifications, and responses over [`serde_json::Value`]. Only
//! this milestone's surface is modeled; parsing is hand-rolled because the
//! `id`-vs-`method` discrimination and the `result`-XOR-`error` response
//! shape are the whole logic.

use serde_json::{Value, json};

/// JSON-RPC error codes (JSON-RPC 2.0 §5.1, LSP §statusCodes).
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
    /// LSP's `RequestCancelled`: the server dropped the request before
    /// producing a result (its enqueue epoch was superseded).
    pub const REQUEST_CANCELLED: i64 = -32800;
}

/// A request id: integer or string (JSON-RPC 2.0); fractional numbers are
/// forbidden by LSP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestId {
    Num(i64),
    Str(String),
}

impl RequestId {
    /// Decodes an `id` field value (`$/cancelRequest` params carry one).
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_i64().map(Self::Num),
            Value::String(text) => Some(Self::Str(text.clone())),
            _ => None,
        }
    }

    fn into_value(self) -> Value {
        match self {
            Self::Num(id) => json!(id),
            Self::Str(id) => json!(id),
        }
    }
}

/// A client→server request: answered with exactly one [`Response`].
#[derive(Debug)]
pub struct Request {
    pub id: RequestId,
    pub method: String,
    pub params: Option<Value>,
}

/// A client→server event: no answer, ever.
#[derive(Debug)]
pub struct Notification {
    pub method: String,
    pub params: Option<Value>,
}

/// A server→client response. `result: None` with `error: None` serializes as
/// `result: null` (the `shutdown` answer).
#[derive(Debug)]
pub struct Response {
    pub id: Option<RequestId>,
    pub result: Option<Value>,
    pub error: Option<(i64, String)>,
}

/// Any message direction; incoming [`Response`]s are ignored by the server
/// (clients do not send them) but kept so the model round-trips.
#[derive(Debug)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

impl Message {
    /// Decodes one JSON-RPC message.
    ///
    /// # Errors
    ///
    /// `Err` is a human-readable reason for an `InvalidRequest` answer.
    pub fn from_value(value: Value) -> Result<Self, String> {
        let Value::Object(object) = value else {
            return Err("message is not a JSON object".into());
        };
        let method = match object.get("method") {
            Some(Value::String(method)) => Some(method.clone()),
            Some(_) => return Err("method must be a string".into()),
            None => None,
        };
        let id = match object.get("id") {
            Some(Value::Null) | None => None,
            Some(raw) => match RequestId::from_value(raw) {
                Some(id) => Some(id),
                None => return Err("id must be an integer or string".into()),
            },
        };
        let params = object.get("params").and_then(|params| {
            // `params: null` and an absent param field are equivalent.
            (!params.is_null()).then(|| params.clone())
        });
        match (method, id) {
            (Some(method), Some(id)) => Ok(Self::Request(Request { id, method, params })),
            (Some(method), None) => Ok(Self::Notification(Notification { method, params })),
            (None, Some(id)) => {
                let error = object.get("error").and_then(|error| {
                    let error = error.as_object()?;
                    Some((
                        error.get("code")?.as_i64()?,
                        error.get("message")?.as_str()?.into(),
                    ))
                });
                Ok(Self::Response(Response {
                    id: Some(id),
                    result: object.get("result").cloned(),
                    error,
                }))
            }
            (None, None) => Err("message has neither method nor id".into()),
        }
    }

    /// Encodes the `jsonrpc: "2.0"` envelope.
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            Self::Request(request) => {
                let mut value = json!({
                    "jsonrpc": "2.0",
                    "id": request.id.clone().into_value(),
                    "method": request.method,
                });
                if let Some(params) = &request.params {
                    value["params"] = params.clone();
                }
                value
            }
            Self::Notification(notification) => {
                let mut value = json!({ "jsonrpc": "2.0", "method": notification.method });
                if let Some(params) = &notification.params {
                    value["params"] = params.clone();
                }
                value
            }
            Self::Response(response) => {
                let mut value = json!({ "jsonrpc": "2.0" });
                value["id"] = match &response.id {
                    Some(id) => id.clone().into_value(),
                    // Error responses to unparseable messages carry a null id.
                    None => Value::Null,
                };
                match &response.error {
                    Some((code, message)) => {
                        value["error"] = json!({ "code": code, "message": message });
                    }
                    // `shutdown` answers `result: null` — the field must be present.
                    None => value["result"] = response.result.clone().unwrap_or(Value::Null),
                }
                value
            }
        }
    }
}

impl Response {
    /// A success response.
    #[must_use]
    pub fn ok(id: RequestId, result: Value) -> Self {
        Self {
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    /// An error response; `id: None` serializes as a JSON `null` id (the
    /// only legal shape when the request could not be decoded).
    #[must_use]
    pub fn error(id: Option<RequestId>, code: i64, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some((code, message.into())),
        }
    }
}

impl Notification {
    /// A server→client notification.
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            method: method.into(),
            params: Some(params),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Message, String> {
        Message::from_value(serde_json::from_str(text).unwrap())
    }

    #[test]
    fn parses_request_with_params() {
        let message = parse(r#"{"jsonrpc":"2.0","id":7,"method":"m","params":{"a":1}}"#).unwrap();
        let Message::Request(request) = message else {
            panic!("request")
        };
        assert_eq!(request.id, RequestId::Num(7));
        assert_eq!(request.method, "m");
        assert_eq!(request.params.unwrap()["a"], 1);
    }

    #[test]
    fn parses_string_ids_and_null_params() {
        let message = parse(r#"{"id":"abc","method":"m","params":null}"#).unwrap();
        let Message::Request(request) = message else {
            panic!("request")
        };
        assert_eq!(request.id, RequestId::Str("abc".into()));
        assert_eq!(request.params, None);
    }

    #[test]
    fn parses_notification_without_id() {
        let message = parse(r#"{"method":"exit"}"#).unwrap();
        let Message::Notification(notification) = message else {
            panic!("notification")
        };
        assert_eq!(notification.method, "exit");
        assert_eq!(notification.params, None);
    }

    #[test]
    fn parses_response_with_error() {
        let message = parse(r#"{"id":1,"error":{"code":-32601,"message":"no"}}"#).unwrap();
        let Message::Response(response) = message else {
            panic!("response")
        };
        assert_eq!(response.id, Some(RequestId::Num(1)));
        assert_eq!(response.error, Some((-32601, "no".into())));
    }

    #[test]
    fn rejects_non_objects_and_idless_methodless_messages() {
        assert!(parse("[]").is_err());
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"id":1.5,"method":"m"}"#).is_err());
        assert!(parse(r#"{"id":true,"method":"m"}"#).is_err());
        assert!(parse(r#"{"id":1,"method":42}"#).is_err());
    }

    #[test]
    fn writes_shutdown_null_result() {
        let response = Message::Response(Response::ok(RequestId::Num(3), Value::Null));
        assert_eq!(
            response.to_value(),
            serde_json::json!({"jsonrpc":"2.0","id":3,"result":null})
        );
    }

    #[test]
    fn writes_error_with_null_id() {
        let response = Message::Response(Response::error(None, codes::PARSE_ERROR, "bad"));
        assert_eq!(
            response.to_value(),
            serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"bad"}})
        );
    }

    #[test]
    fn writes_notification_without_empty_params() {
        let message = Message::Notification(Notification::new("exit", serde_json::json!({})));
        assert_eq!(
            message.to_value(),
            serde_json::json!({"jsonrpc":"2.0","method":"exit","params":{}})
        );
    }

    #[test]
    fn request_round_trip() {
        let text = r#"{"jsonrpc":"2.0","id":9,"method":"x","params":[1]}"#;
        let message = parse(text).unwrap();
        assert_eq!(
            message.to_value(),
            serde_json::from_str::<Value>(text).unwrap()
        );
    }
}
