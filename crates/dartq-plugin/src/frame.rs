//! Wire frames and the newline-delimited JSON codec.
//!
//! One frame is one JSON object on one line, UTF-8 encoded, terminated by
//! `'\n'`, and at most [`MAX_FRAME_SIZE`] bytes long:
//!
//! ```json
//! {"schema_version": 1, "id": 7, "method": "initialize", "params": {}}
//! ```
//!
//! `id` is `null` for notifications (plugin-to-core) and an integer for
//! requests and replies (core-to-plugin and their responses). The full
//! contract — method table, handshake sequence, payload shapes — is specified
//! in the crate documentation.

use memchr::memchr;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The protocol version this crate speaks. The `initialize` handshake
/// fails with [`ProtocolError::SchemaVersion`] on any other version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Hard upper bound for one frame, enforced before buffering: the decoder
/// never holds more than this many bytes, keeping host memory bounded when a
/// plugin misbehaves (PRD §14.3 resource isolation).
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Core-to-plugin: the opening handshake request.
pub const METHOD_INITIALIZE: &str = "initialize";
/// Core-to-plugin: the final request; the session ends after its reply.
pub const METHOD_SHUTDOWN: &str = "shutdown";
/// Plugin-to-core notification: diagnostics produced by the plugin.
pub const METHOD_DIAGNOSTIC_NOTIFICATION: &str = "notification/diagnostic";
/// Core-to-plugin notification: the engine's current diagnostics for the
/// analyzed set, pushed once per analyze cycle as the plugin's diagnostic
/// context (PRD §14.2: plugins never hold core references — this batch is
/// the push half of that contract).
pub const METHOD_ENGINE_DIAGNOSTICS: &str = "notification/engine_diagnostics";
/// Core-to-plugin: request source fixes for a location.
pub const METHOD_REQUEST_FIXES: &str = "request/fixes";

/// One wire frame: a request, a reply, or a notification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// Protocol version; must equal [`PROTOCOL_VERSION`].
    pub schema_version: u32,
    /// Request/reply correlation id; `None` marks a notification.
    #[serde(default)]
    pub id: Option<u64>,
    /// The method name (see the `METHOD_*` constants).
    pub method: String,
    /// Method-specific payload; always a JSON object on the wire.
    #[serde(default)]
    pub params: Map<String, Value>,
}

impl Frame {
    /// Builds a request frame (carrying a correlation `id`).
    #[must_use]
    pub fn request(id: u64, method: &str, params: Map<String, Value>) -> Self {
        Self {
            schema_version: PROTOCOL_VERSION,
            id: Some(id),
            method: method.to_owned(),
            params,
        }
    }

    /// Builds a notification frame (no `id`).
    #[must_use]
    pub fn notification(method: &str, params: Map<String, Value>) -> Self {
        Self {
            schema_version: PROTOCOL_VERSION,
            id: None,
            method: method.to_owned(),
            params,
        }
    }

    /// Whether this frame is a notification (no correlation `id`).
    #[must_use]
    pub const fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// Codec-level failures. Any of these means the wire contract is broken; the
/// caller must tear the plugin process down (PRD §14.3 crash isolation) —
/// the codec never attempts resynchronization.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// A frame exceeded [`MAX_FRAME_SIZE`].
    #[error("frame exceeds the {limit}-byte frame limit")]
    TooLarge {
        /// The enforced limit.
        limit: usize,
    },
    /// A frame was not valid UTF-8.
    #[error("frame is not valid UTF-8: {0}")]
    Utf8(String),
    /// A frame was not valid JSON.
    #[error("frame is not valid JSON: {0}")]
    Json(String),
    /// A frame advertised a `schema_version` this build does not speak.
    #[error("unsupported schema_version {got}; this build speaks version {expected}")]
    SchemaVersion {
        /// The version the frame carried.
        got: u32,
        /// The version this build speaks ([`PROTOCOL_VERSION`]).
        expected: u32,
    },
    /// A frame carried an empty `method`.
    #[error("frame `method` must be a non-empty string")]
    EmptyMethod,
}

/// Serializes one frame to a single JSON line, **without** the trailing
/// `'\n'`; the caller appends it when writing to a stream.
///
/// # Errors
///
/// Returns [`ProtocolError::Json`] if the params contain values that are not
/// representable in JSON.
pub fn encode_frame(frame: &Frame) -> Result<String, ProtocolError> {
    serde_json::to_string(frame).map_err(|e| ProtocolError::Json(e.to_string()))
}

/// Decodes exactly one frame from one line's bytes (trailing `'\n'` already
/// stripped).
///
/// # Errors
///
/// Returns a typed [`ProtocolError`] for over-long frames, non-UTF-8 bytes,
/// malformed JSON, a foreign `schema_version`, or an empty `method`.
pub fn decode_line(line: &[u8]) -> Result<Frame, ProtocolError> {
    if line.len() > MAX_FRAME_SIZE {
        return Err(ProtocolError::TooLarge {
            limit: MAX_FRAME_SIZE,
        });
    }
    let text = std::str::from_utf8(line).map_err(|e| ProtocolError::Utf8(e.to_string()))?;
    let frame: Frame =
        serde_json::from_str(text).map_err(|e| ProtocolError::Json(e.to_string()))?;
    if frame.schema_version != PROTOCOL_VERSION {
        return Err(ProtocolError::SchemaVersion {
            got: frame.schema_version,
            expected: PROTOCOL_VERSION,
        });
    }
    if frame.method.is_empty() {
        return Err(ProtocolError::EmptyMethod);
    }
    Ok(frame)
}

/// Incremental frame decoder for a byte stream (e.g. a plugin's stdout).
///
/// Feed arbitrary chunks with [`FrameDecoder::push`]; complete frames come
/// back as they terminate. Memory is bounded by [`MAX_FRAME_SIZE`]: if the
/// stream produces a frame beyond it, the buffer is dropped and a typed error
/// is returned — the caller must tear the connection down, and the decoder
/// must be discarded.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    /// Feeds raw stream bytes and returns every complete frame they contain.
    /// Blank lines are skipped; a `'\r'` before `'\n'` is stripped so CRLF
    /// streams decode identically.
    ///
    /// # Errors
    ///
    /// Returns the first [`ProtocolError`] any decoded line fails with. On
    /// error the internal buffer is cleared (bounded-memory guarantee); the
    /// decoder must not be reused.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, ProtocolError> {
        if self.buf.len() + bytes.len() > MAX_FRAME_SIZE {
            self.buf.clear();
            return Err(ProtocolError::TooLarge {
                limit: MAX_FRAME_SIZE,
            });
        }
        self.buf.extend_from_slice(bytes);

        let mut frames = Vec::new();
        let mut consumed = 0;
        while let Some(offset) = memchr(b'\n', &self.buf[consumed..]) {
            let end = consumed + offset;
            let line = &self.buf[consumed..end];
            let line = if line.last() == Some(&b'\r') {
                &line[..line.len() - 1]
            } else {
                line
            };
            consumed = end + 1;
            if !line.is_empty() {
                match decode_line(line) {
                    Ok(frame) => frames.push(frame),
                    Err(e) => {
                        self.buf.clear();
                        return Err(e);
                    }
                }
            }
        }
        self.buf.drain(..consumed);
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_request() -> Frame {
        let mut params = Map::new();
        params.insert("file".into(), json!("lib/main.dart"));
        params.insert("offset".into(), json!(12));
        Frame::request(7, METHOD_REQUEST_FIXES, params)
    }

    fn sample_notification() -> Frame {
        let mut params = Map::new();
        params.insert("file".into(), json!("lib/main.dart"));
        params.insert("severity".into(), json!("warning"));
        params.insert("code".into(), json!("my_plugin/lint"));
        params.insert("message".into(), json!("prefer const"));
        Frame::notification(METHOD_DIAGNOSTIC_NOTIFICATION, params)
    }

    #[test]
    fn frame_round_trips_through_encode_and_decode() {
        for frame in [sample_request(), sample_notification()] {
            let line = encode_frame(&frame).expect("encode");
            assert!(!line.contains('\n'));
            let decoded = decode_line(line.as_bytes()).expect("decode");
            assert_eq!(decoded, frame);
        }
    }

    #[test]
    fn missing_fields_decode_to_typed_defaults() {
        // `id: null` + missing `params` is a bare notification.
        let frame = decode_line(br#"{"schema_version":1,"id":null,"method":"x"}"#).expect("decode");
        assert!(frame.is_notification());
        assert!(frame.params.is_empty());
    }

    #[test]
    fn malformed_json_is_rejected_with_typed_error() {
        let err = decode_line(br#"{"schema_version":1,"method":"#).unwrap_err();
        assert!(matches!(err, ProtocolError::Json(_)));
        // Valid JSON of the wrong shape (a bare array) is malformed too.
        let err = decode_line(b"[1,2,3]").unwrap_err();
        assert!(matches!(err, ProtocolError::Json(_)));
    }

    #[test]
    fn foreign_schema_version_and_empty_method_are_rejected() {
        let err = decode_line(br#"{"schema_version":2,"method":"x"}"#).unwrap_err();
        assert_eq!(
            err,
            ProtocolError::SchemaVersion {
                got: 2,
                expected: PROTOCOL_VERSION
            }
        );
        let err = decode_line(br#"{"schema_version":1,"method":""}"#).unwrap_err();
        assert_eq!(err, ProtocolError::EmptyMethod);
    }

    #[test]
    fn non_utf8_bytes_are_rejected() {
        let err = decode_line(&[0xff, 0xfe, 0x0a]).unwrap_err();
        assert!(matches!(err, ProtocolError::Utf8(_)));
    }

    #[test]
    fn oversize_frame_is_rejected_before_allocation() {
        let oversized = vec![b'a'; MAX_FRAME_SIZE + 1];
        let err = decode_line(&oversized).unwrap_err();
        assert_eq!(
            err,
            ProtocolError::TooLarge {
                limit: MAX_FRAME_SIZE
            }
        );
    }

    #[test]
    fn decoder_splits_chunks_into_frames_and_skips_blank_lines() {
        let a = encode_frame(&sample_request()).expect("encode");
        let b = encode_frame(&sample_notification()).expect("encode");
        let mut decoder = FrameDecoder::default();
        // Feed one byte at a time to prove partial frames buffer correctly.
        let mut frames = Vec::new();
        for byte in format!("{a}\n\n{b}\n").into_bytes() {
            frames.extend(decoder.push(&[byte]).expect("push"));
        }
        assert_eq!(frames, vec![sample_request(), sample_notification()]);
        assert!(decoder.buf.is_empty(), "no partial frame must remain");
    }

    #[test]
    fn decoder_rejects_oversize_stream_and_drops_buffer() {
        let mut decoder = FrameDecoder::default();
        let err = decoder.push(&vec![b'x'; MAX_FRAME_SIZE + 1]).unwrap_err();
        assert_eq!(
            err,
            ProtocolError::TooLarge {
                limit: MAX_FRAME_SIZE
            }
        );
        assert!(decoder.buf.is_empty(), "buffer must be dropped on error");
    }
}
