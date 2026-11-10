//! Significant-content signatures for Dart sources.
//!
//! A cache key must change when a file's meaning changes and stay put when
//! it does not. Raw bytes over-approximate meaning: a reworded comment, a
//! reflowed line or a format-only pass changes the byte digest and
//! invalidates every suite whose closure reaches the file, though the
//! program the compiler reads — the token stream — is the same.
//!
//! [`dart_signature`] hashes the dartq scanner's token table instead:
//! comments and non-semantic whitespace are gone by construction (the
//! scanner emits no tokens for either), every string lexeme is one token
//! kept verbatim (whitespace inside a literal is the literal's own), and
//! interpolation segments hash in source order. Two files whose token
//! streams match hash alike, so a comment-only or format-only edit re-keys
//! nothing, while any change that reaches the compiler — an identifier, a
//! literal, an operator, a keyword — re-keys everything downstream.
//!
//! ## What the signature deliberately keeps
//!
//! - Every token's lexeme exactly as spelled: quote styles and raw
//!   prefixes are content (`'a'`, `"a"` and `r'a'` hash differently), and
//!   so are number spellings (`0xFF` and `255`). The signature is an
//!   equivalence over token streams, not over program semantics — it is
//!   never *less* conservative than the compiler's own view of the file.
//! - Recovery artifacts: an unterminated construct recovers
//!   deterministically from the bytes, so its synthetic tokens belong to
//!   the stream's identity.
//!
//! ## What it does not claim
//!
//! A suite that READS a Dart file as data (a fixture analyzed or compared
//! by its bytes) observes what the signature strips: the cache keys such
//! files on their raw digest (the data-role leaves in [`crate::cache`]),
//! and only files the suite reaches as compiled code key on this
//! signature. [`crate::deps`] facts — markers, path literals, drifting
//! reads — are likewise extracted from the raw source and keep taking
//! effect on comment-only edits.

use dartq_scanner::{ScanConfig, TokenKind, TokenTable};
use dartq_source::FileTable;
use std::path::Path;

/// Domain separator: the digest names the token-stream shape it hashes,
/// and never collides with a raw-content digest folded into the same key.
const DOMAIN: &[u8] = b"dartq-tokensig-v1";

/// The scan configuration every dartq-test scan shares: the language
/// features of the package's own resolution as [`crate::deps::facts`]
/// scans it, so a file's facts and its signature always agree on what its
/// tokens are.
fn scan_config() -> ScanConfig {
    ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: false,
    }
}

/// The significant-content signature of one Dart file's bytes: a blake3
/// digest over its scanned token stream (see the module docs). `None` when
/// the bytes are not UTF-8 — a Dart source that cannot decode never
/// compiles, and the caller falls back to keying the raw bytes.
#[must_use]
pub fn dart_signature(bytes: &[u8]) -> Option<[u8; 32]> {
    let text = std::str::from_utf8(bytes).ok()?;
    let source = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut files = FileTable::new();
    let scanned = dartq_scanner::scan(
        source,
        scan_config(),
        files.intern(Path::new("source.dart")),
    );
    Some(signature_of(source, &scanned.tokens))
}

/// The digest of one scanned token table.
fn signature_of(source: &str, tokens: &TokenTable) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN);
    for token in tokens.as_slice() {
        match token.kind {
            // The end-of-file sentinel carries no content.
            TokenKind::Eof => break,
            kind @ (TokenKind::Identifier
            | TokenKind::Keyword(_)
            | TokenKind::Int
            | TokenKind::Double
            | TokenKind::Hexadecimal
            | TokenKind::String
            | TokenKind::ScriptTag) => {
                hasher.update(&[sliceable_tag(kind)]);
                let lexeme = &source[token.byte_range()];
                hasher.update(&(lexeme.len() as u64).to_le_bytes());
                hasher.update(lexeme.as_bytes());
            }
            // Every other kind's identity is its fixed lexeme, unique per
            // kind; no sliceable token can share it (identifiers, numbers,
            // strings and script tags never match an operator's spelling).
            kind => {
                hasher.update(&[0]);
                let fixed = kind.fixed_lexeme().unwrap_or_default();
                hasher.update(&(fixed.len() as u64).to_le_bytes());
                hasher.update(fixed.as_bytes());
            }
        }
    }
    *hasher.finalize().as_bytes()
}

/// The signature's tag byte for the sliceable kinds, each distinct so two
/// kinds can never collide on one lexeme (`1` an int, a string `"1"`).
fn sliceable_tag(kind: TokenKind) -> u8 {
    match kind {
        TokenKind::Identifier => b'i',
        TokenKind::Keyword(_) => b'k',
        TokenKind::Int => b'n',
        TokenKind::Double => b'f',
        TokenKind::Hexadecimal => b'x',
        TokenKind::String => b's',
        TokenKind::ScriptTag => b't',
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(source: &str) -> [u8; 32] {
        dart_signature(source.as_bytes()).expect("utf-8")
    }

    #[test]
    fn comment_and_whitespace_edits_keep_the_signature() {
        let plain = "int add(int a, int b) => a + b;\n";
        let commented = concat!(
            "// a comment that says nothing\n",
            "/* block */ int add(int a, /* mid */ int b) => a + b; /// doc\n",
            "\n\n\t",
        );
        assert_eq!(sig(plain), sig(commented));
        // Same statement, reflowed across lines and indentation.
        assert_eq!(
            sig("void main() { f(a, b); }\n"),
            sig("void main() {\n  f(\n    a,\n    b\n  );\n}\n")
        );
        // A BOM is not content either.
        assert_eq!(sig(plain), sig(&format!("\u{feff}{plain}")));
        // A file of comments only means nothing to the compiler: it signs
        // like an empty file.
        assert_eq!(sig(""), sig("// just a comment\n"));
    }

    #[test]
    fn significant_edits_change_the_signature() {
        let base = "int add(int a, int b) => a + b;\n";
        assert_ne!(sig(base), sig("int add(int a, int b) => a - b;\n"));
        assert_ne!(sig(base), sig("int add2(int a, int b) => a + b;\n"));
        // String content is content, whitespace inside included.
        assert_ne!(sig("var s = 'a b';\n"), sig("var s = 'a  b';\n"));
        assert_ne!(sig("var s = 'a b';\n"), sig("var s = 'a c';\n"));
        // Interpolation members are content; whitespace between the
        // interpolated expression's tokens is not.
        assert_ne!(sig("var s = 'a${b}c';\n"), sig("var s = 'a${d}c';\n"));
        assert_eq!(sig("var s = 'a${b}c';\n"), sig("var s = 'a${\n  b\n}c';\n"));
        // Keyword vs identifier, number spellings, quote styles.
        assert_ne!(sig("var x = 1;\n"), sig("final x = 1;\n"));
        assert_ne!(sig("var x = 255;\n"), sig("var x = 0xFF;\n"));
        assert_ne!(sig("var s = 'a';\n"), sig(r#"var s = "a";"#));
    }

    #[test]
    fn non_utf8_sources_have_no_signature() {
        assert!(dart_signature(&[0xff, 0xfe, b'x']).is_none());
    }

    /// The signature is stable against token-table internals: the same
    /// source signs alike across calls, and recovery artifacts belong to
    /// the stream (an unterminated string still distinguishes its files).
    #[test]
    fn signatures_are_stable_and_recovery_aware() {
        let source = "void main() { print('hi'); }\n";
        assert_eq!(sig(source), sig(source));
        assert_ne!(
            sig("var s = 'unterminated;\n"),
            sig("var s = 'unterminatedx';\n")
        );
    }
}
