//! LSP wire framing: `Content-Length: N\r\n\r\n` headers + one JSON body
//! (LSP 3.17 §baseProtocol). Hand-written because the shell carries no Dart
//! semantics; the format is ~40 lines and pinned by the tests below.

use std::io::{self, BufRead, Write};

/// Upper bound for one message body. A client is also bounded by this when
/// sending documents; unbounded `Content-Length` values would let a broken
/// peer allocate without limit.
pub const MAX_FRAME_BYTES: usize = 256 << 20;

/// One frame read failure.
#[derive(Debug)]
pub enum FrameError {
    /// The underlying stream failed (peer vanished, IO error).
    Io(io::Error),
    /// The framing itself is broken; byte alignment is lost and the stream
    /// cannot be re-synchronized — the session must end.
    Protocol(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "input stream failed: {error}"),
            Self::Protocol(message) => write!(f, "protocol violation: {message}"),
        }
    }
}

/// Reads one frame body. `Ok(None)` is a clean end of stream at a message
/// boundary; any other error means the session cannot continue.
///
/// # Errors
///
/// [`FrameError::Io`] when the stream fails mid-frame; [`FrameError::Protocol`]
/// when the framing is broken (missing or invalid `Content-Length`, malformed
/// header line, truncation) and the byte stream cannot be re-synchronized.
pub fn read_frame(input: &mut impl BufRead) -> Result<Option<Vec<u8>>, FrameError> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = Vec::new();
        let read = input.read_until(b'\n', &mut line).map_err(FrameError::Io)?;
        if read == 0 {
            return match content_length {
                // EOF exactly at a message boundary.
                None => Ok(None),
                Some(_) => Err(FrameError::Protocol("truncated header block".into())),
            };
        }
        // Accept both CRLF and bare LF line endings.
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            // Blank line ends the header block.
            let length = content_length
                .ok_or_else(|| FrameError::Protocol("missing Content-Length header".into()))?;
            let mut body = vec![0; length];
            input.read_exact(&mut body).map_err(FrameError::Io)?;
            return Ok(Some(body));
        }
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return Err(FrameError::Protocol("malformed header line".into()));
        };
        let (name, value) = (&line[..colon], &line[colon + 1..]);
        if name.eq_ignore_ascii_case(b"content-length") {
            let text = std::str::from_utf8(value)
                .map_err(|_| FrameError::Protocol("non-UTF-8 header value".into()))?;
            let length: usize = text
                .trim()
                .parse()
                .map_err(|_| FrameError::Protocol("invalid Content-Length".into()))?;
            if length > MAX_FRAME_BYTES {
                return Err(FrameError::Protocol(
                    "Content-Length exceeds the 256 MiB limit".into(),
                ));
            }
            content_length = Some(length);
        }
        // Other headers (Content-Type) are legal and skipped.
    }
}

/// Writes one framed message and flushes — every message must reach the
/// client immediately (diagnostics are pushes, not batched output).
///
/// # Errors
///
/// Propagates write and flush failures from `output`.
pub fn write_frame(output: &mut impl Write, body: &[u8]) -> io::Result<()> {
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(body)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Reader that yields at most `chunk` bytes per `read`, forcing the
    /// [`BufRead`] layer to reassemble partial reads (half-delivered frames).
    struct Trickle {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let take = buf.len().min(self.chunk).min(self.data.len() - self.pos);
            buf[..take].copy_from_slice(&self.data[self.pos..self.pos + take]);
            self.pos += take;
            Ok(take)
        }
    }

    fn frame(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    }

    #[test]
    fn reads_one_frame() {
        let mut input = io::Cursor::new(frame(r#"{"x":1}"#).into_bytes());
        let body = read_frame(&mut input).unwrap().unwrap();
        assert_eq!(body, br#"{"x":1}"#);
        assert_eq!(read_frame(&mut input).unwrap(), None);
    }

    #[test]
    fn reads_two_stuck_together_frames() {
        let input = frame("first") + &frame("second-longer");
        let mut input = io::Cursor::new(input.into_bytes());
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"first");
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"second-longer");
        assert_eq!(read_frame(&mut input).unwrap(), None);
    }

    #[test]
    fn reads_frames_delivered_in_pieces() {
        let data = (frame("alpha") + &frame("beta gamma")).into_bytes();
        let mut input = io::BufReader::new(Trickle {
            data,
            pos: 0,
            chunk: 3,
        });
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"alpha");
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"beta gamma");
        assert_eq!(read_frame(&mut input).unwrap(), None);
    }

    #[test]
    fn accepts_bare_lf_headers() {
        let mut input = "Content-Length: 2\n\nok".as_bytes();
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"ok");
    }

    #[test]
    fn skips_other_headers_and_is_case_insensitive() {
        let text = "content-type: application/vscode-jsonrpc; charset=utf-8\r\n\
                    CONTENT-LENGTH: 2\r\n\r\nhi";
        let mut input = text.as_bytes();
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"hi");
    }

    #[test]
    fn missing_content_length_is_protocol_error() {
        let mut input = io::Cursor::new(b"Content-Type: text/plain\r\n\r\nhello".to_vec());
        assert!(matches!(
            read_frame(&mut input),
            Err(FrameError::Protocol(_))
        ));
    }

    #[test]
    fn malformed_header_is_protocol_error() {
        let mut input = io::Cursor::new(b"no colon here\r\n\r\n".to_vec());
        assert!(matches!(
            read_frame(&mut input),
            Err(FrameError::Protocol(_))
        ));
    }

    #[test]
    fn non_numeric_length_is_protocol_error() {
        let mut input = io::Cursor::new(b"Content-Length: many\r\n\r\n".to_vec());
        assert!(matches!(
            read_frame(&mut input),
            Err(FrameError::Protocol(_))
        ));
    }

    #[test]
    fn oversize_length_is_rejected_before_allocation() {
        let mut input = io::Cursor::new(
            format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1).into_bytes(),
        );
        assert!(matches!(
            read_frame(&mut input),
            Err(FrameError::Protocol(_))
        ));
    }

    #[test]
    fn eof_inside_header_block_is_truncation() {
        let mut input = io::Cursor::new(b"Content-Length: 10\r\n".to_vec());
        assert!(matches!(
            read_frame(&mut input),
            Err(FrameError::Protocol(_))
        ));
    }

    #[test]
    fn eof_inside_body_is_io_error() {
        let mut input = io::Cursor::new(b"Content-Length: 10\r\n\r\nshort".to_vec());
        assert!(matches!(read_frame(&mut input), Err(FrameError::Io(_))));
    }

    #[test]
    fn written_frame_is_exact_bytes() {
        let mut output = Vec::new();
        write_frame(&mut output, b"body").unwrap();
        assert_eq!(output, b"Content-Length: 4\r\n\r\nbody");
    }

    #[test]
    fn written_and_read_round_trip() {
        let mut output = Vec::new();
        write_frame(&mut output, br#"{"a":null}"#).unwrap();
        write_frame(&mut output, b"two").unwrap();
        let mut input = io::Cursor::new(output);
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), br#"{"a":null}"#);
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"two");
    }
}
