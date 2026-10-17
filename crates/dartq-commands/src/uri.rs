//! `file:` URI ↔ filesystem path mapping — the daemon's overlay keys and
//! the document-URI rows the analyze faces serve.
//!
//! Hand-rolled because the scheme space both consumers accept is exactly
//! one (`file:`), and a full URL crate would buy nothing on the
//! single-client milestone. Non-`file:` schemes (e.g. `untitled:`) have no
//! path and are reported as [`to_path`] → `None`; the server skips such
//! documents.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Maps a `file:` URI to its path. Accepts an empty or `localhost` authority
/// (the two local forms clients emit) and cuts at the first `#` fragment
/// marker — [`from_path`] percent-encodes literal `#`, so any bare `#` is a
/// fragment. Returns `None` for other schemes and remote authorities.
#[must_use]
pub fn to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // The slash that separates authority from path belongs to the path:
    // `file:///x` has an empty authority and the absolute path `/x`.
    let (authority, path) = match rest.split_once('/') {
        Some((authority, _)) => (authority, &rest[authority.len()..]),
        None => (rest, ""),
    };
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let path = path.split('#').next().unwrap_or(path);
    let bytes = percent_decode(path);
    Some(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Maps a path to a percent-encoded `file:` URI. Unreserved bytes and `/`
/// pass through; every other byte becomes `%XX` over its UTF-8 encoding.
#[must_use]
pub fn from_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut uri = String::with_capacity(text.len() + 8);
    uri.push_str("file://");
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                uri.push(*byte as char);
            }
            _ => {
                let _ = write!(uri, "%{byte:02X}");
            }
        }
    }
    uri
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escape = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|pair| match (hex_nibble(pair[0]), hex_nibble(pair[1])) {
                (Some(high), Some(low)) => Some(high * 16 + low),
                _ => None,
            });
        if let Some(byte) = escape {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_plain_paths() {
        let path = PathBuf::from("/tmp/dartq/src/main.dart");
        assert_eq!(to_path(&from_path(&path)), Some(path));
    }

    #[test]
    fn encodes_spaces_and_unicode() {
        assert_eq!(
            from_path(Path::new("/a b/δ.dart")),
            "file:///a%20b/%CE%B4.dart"
        );
        assert_eq!(
            to_path("file:///a%20b/%CE%B4.dart"),
            Some(PathBuf::from("/a b/δ.dart"))
        );
    }

    #[test]
    fn encodes_hash_so_bare_hash_is_a_fragment() {
        assert_eq!(from_path(Path::new("/a#b.dart")), "file:///a%23b.dart");
        assert_eq!(
            to_path("file:///a.dart#frag"),
            Some(PathBuf::from("/a.dart"))
        );
    }

    #[test]
    fn accepts_empty_and_localhost_authorities() {
        assert_eq!(
            to_path("file:///x/y.dart"),
            Some(PathBuf::from("/x/y.dart"))
        );
        assert_eq!(
            to_path("file://localhost/x/y.dart"),
            Some(PathBuf::from("/x/y.dart"))
        );
    }

    #[test]
    fn rejects_other_schemes_and_remote_hosts() {
        assert_eq!(to_path("untitled:Untitled-1"), None);
        assert_eq!(to_path("http://host/x.dart"), None);
        assert_eq!(to_path("file://remote/x.dart"), None);
    }

    #[test]
    fn malformed_percent_escapes_pass_through() {
        assert_eq!(
            to_path("file:///a%zz.dart"),
            Some(PathBuf::from("/a%zz.dart"))
        );
        assert_eq!(to_path("file:///a%.dart"), Some(PathBuf::from("/a%.dart")));
    }
}
