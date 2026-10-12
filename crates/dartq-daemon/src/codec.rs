//! LSP wire framing: `Content-Length: N\r\n\r\n` headers + one JSON body
//! (LSP 3.17 §baseProtocol). Hand-written because the shell carries no Dart
//! semantics; the format is ~40 lines and pinned by the tests below.

use std::io::{self, BufRead, Read, Write};

/// Upper bound for one header line (#947 F1): LSP headers are tiny, and
/// without a ceiling a peer streaming newline-free bytes would grow the
/// line buffer without limit and take the process down before any framing
/// check fires. 16 KiB is the industry-standard ceiling; it bounds the
/// content, with the line terminator on top of the cap.
const MAX_HEADER_LINE: usize = 16 * 1024;

/// Upper bound for one message body (#947 F2): 16 MiB, the same wire cap
/// the plugin transport enforces (`dartq-plugin`'s `MAX_FRAME_SIZE`) —
/// bodies carry whole documents and real Dart sources sit far below this.
/// Unbounded `Content-Length` values would let a broken peer allocate
/// without limit.
pub const MAX_FRAME_BYTES: usize = 16 << 20;

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
/// when the framing is broken (missing, duplicate or invalid
/// `Content-Length`, malformed header line, a truncated header block, a
/// truncated body) and the byte stream cannot be re-synchronized.
pub fn read_frame(input: &mut impl BufRead) -> Result<Option<Vec<u8>>, FrameError> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = Vec::new();
        // The `take` bounds the read so a newline-free peer stream cannot
        // grow the buffer without limit (#947 F1); the +1 lets a terminator
        // land, so an exactly-maximal legal line is still recognized. The
        // reader is reborrowed (fully qualified call), so bytes beyond the
        // cap stay buffered in `input` for the following reads.
        let read = Read::take(&mut *input, MAX_HEADER_LINE as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(FrameError::Io)?;
        if read == 0 {
            return match content_length {
                // EOF exactly at a message boundary.
                None => Ok(None),
                Some(_) => Err(FrameError::Protocol("truncated header block".into())),
            };
        }
        if line.len() > MAX_HEADER_LINE && line.last() != Some(&b'\n') {
            // The cap filled with no terminator in sight: the peer is
            // streaming an unbounded header line. Byte alignment is lost —
            // the session ends.
            return Err(FrameError::Protocol(
                "header line exceeds the 16 KiB limit".into(),
            ));
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
            if let Err(error) = input.read_exact(&mut body) {
                // A short body is a framing violation, not a stream failure
                // (#947 F10): the peer declared `length` bytes and stopped
                // delivering — supervision must be able to tell that from a
                // clean close, so it ends the session as Protocol (the
                // desync path, non-zero exit) instead of Io (logged as a
                // vanished peer). A genuine mid-read IO error stays Io.
                return Err(match error.kind() {
                    io::ErrorKind::UnexpectedEof => FrameError::Protocol(format!(
                        "frame body truncated: {length} bytes declared, \
                         the stream ended early"
                    )),
                    _ => FrameError::Io(error),
                });
            }
            return Ok(Some(body));
        }
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return Err(FrameError::Protocol("malformed header line".into()));
        };
        let (name, value) = (&line[..colon], &line[colon + 1..]);
        if name.eq_ignore_ascii_case(b"content-length") {
            // One Content-Length per message (LSP §baseProtocol header
            // semantics, #947 F11): a repeat is a framing violation — the
            // two declarations cannot both describe the one body, and
            // last-wins would silently pick the peer's second word over
            // its first.
            if content_length.is_some() {
                return Err(FrameError::Protocol(
                    "duplicate Content-Length header".into(),
                ));
            }
            let text = std::str::from_utf8(value)
                .map_err(|_| FrameError::Protocol("non-UTF-8 header value".into()))?;
            let length: usize = text
                .trim()
                .parse()
                .map_err(|_| FrameError::Protocol("invalid Content-Length".into()))?;
            if length > MAX_FRAME_BYTES {
                return Err(FrameError::Protocol(
                    "Content-Length exceeds the 16 MiB limit".into(),
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

    /// A newline-free peer stream must hit the header-line ceiling and end
    /// as a protocol error, never grow the line buffer without bound
    /// (#947 F1 — the OOM shape).
    #[test]
    fn oversized_header_line_is_protocol_error() {
        let mut input = io::Cursor::new(vec![b'x'; MAX_HEADER_LINE * 2]);
        match read_frame(&mut input) {
            Err(FrameError::Protocol(message)) => {
                assert!(message.contains("16 KiB"), "{message}");
            }
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    /// Header lines at the ceiling are legal: the ceiling counts the
    /// physical line including its terminator, so a bare-LF line carries
    /// cap content bytes and a CRLF line one fewer (the +1 in the bounded
    /// read must be able to see the terminator to tell legal from overflow).
    #[test]
    fn header_line_at_the_cap_is_accepted() {
        // "Content-Length: 2" is 17 bytes; pad the value with spaces (the
        // parser trims) to pin the exact length.
        let bare_lf = format!("Content-Length: 2{:1$}\n\nok", "", MAX_HEADER_LINE - 17);
        let mut input = io::Cursor::new(bare_lf.into_bytes());
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"ok");

        let crlf = format!("Content-Length: 2{:1$}\r\n\r\nok", "", MAX_HEADER_LINE - 18);
        let mut input = io::Cursor::new(crlf.into_bytes());
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"ok");
    }

    /// A body shorter than its declared `Content-Length` is a framing
    /// violation, not a stream failure (#947 F10): it must surface as
    /// `Protocol` so the session ends on the desync path (non-zero exit)
    /// instead of reading as a clean close.
    #[test]
    fn truncated_body_is_protocol_error() {
        let mut input = io::Cursor::new(b"Content-Length: 10\r\n\r\nshort".to_vec());
        match read_frame(&mut input) {
            Err(FrameError::Protocol(message)) => {
                assert!(message.contains("truncated"), "{message}");
            }
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    /// A repeated `Content-Length` header is a framing violation (#947
    /// F11): last-wins would silently re-frame the body.
    #[test]
    fn duplicate_content_length_is_protocol_error() {
        let mut input =
            io::Cursor::new(b"Content-Length: 2\r\nContent-Length: 3\r\n\r\nabc".to_vec());
        assert!(matches!(
            read_frame(&mut input),
            Err(FrameError::Protocol(_))
        ));
    }

    /// A repeated header that is not `Content-Length` stays legal — the
    /// duplicate rule is about the one header that frames the body.
    #[test]
    fn duplicate_other_header_is_accepted() {
        let mut input = io::Cursor::new(
            b"Content-Type: a\r\nContent-Type: b\r\nContent-Length: 2\r\n\r\nok".to_vec(),
        );
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), b"ok");
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
