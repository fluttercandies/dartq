//! Comment extraction from the inter-token gaps of one parsed unit.
//!
//! The scanner keeps comments out of the token table (officially they form a
//! `CommentToken` chain attached to the preceding token and never enter the
//! main stream — `dartq-scanner` crate docs "Comments"). Lints need them for
//! two things: suppression comments (`// ignore:` family, ported in
//! [`crate::ignore`]) and the "no comments before the closing brace"
//! conditions of `empty_constructor_bodies` / `empty_catches`. Both are
//! served by one linear pass over the gaps between consecutive tokens: every
//! byte between two adjacent tokens is trivia, so a `//` or `/*` found there
//! is exactly a comment — a `//` inside a string literal always lies inside
//! one token's byte range and is never scanned. Block comments nest (depth
//! counted, matching the scanner; `dartq-scanner` crate docs).

use dartq_scanner::TokenTable;

/// A comment found between two tokens.
#[derive(Clone, Debug)]
pub struct Comment {
    /// Byte range `[start, end)` of the whole lexeme (`//`-line comments end
    /// before the line terminator, matching the official comment lexeme).
    pub range: core::ops::Range<u32>,
    pub kind: CommentKind,
    /// Index of the token this comment is attached to (the official
    /// `precedingComments` chain head; comments between token *i* and token
    /// *i+1* attach to *i+1*).
    pub token: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentKind {
    /// `// ...` (includes `///` and `// ignore:` forms).
    Line,
    /// `/* ... */`, nesting-aware.
    Block,
}

/// All comments of one unit in source order, grouped by attachment token.
#[derive(Debug, Default)]
pub struct Comments {
    comments: Vec<Comment>,
    /// Per token index: index of the first comment attached to that token, or
    /// [`NONE`].
    first_attached: Vec<u32>,
}

/// Sentinel for "no comment attached".
pub const NONE: u32 = u32::MAX;

impl Comments {
    /// Extracts the comments of one scanned+parsed unit. The token table must
    /// be the unified post-parse table (the same sequencing contract as
    /// `dartq_ast::byte_span`).
    #[must_use]
    pub fn extract(source: &str, tokens: &TokenTable) -> Self {
        let bytes = source.as_bytes();
        let table = tokens.as_slice();
        let mut comments: Vec<Comment> = Vec::new();
        let mut first_attached = vec![NONE; table.len()];

        let mut cursor = 0usize; // byte offset where the current gap starts
        for (i, token) in table.iter().enumerate() {
            let gap_end = token.byte_offset as usize;
            let mut pos = cursor.min(gap_end);
            while pos < gap_end {
                match bytes[pos] {
                    b'/' if bytes.get(pos + 1) == Some(&b'/') => {
                        let body = &bytes[pos + 2..gap_end];
                        let end = pos + 2 + memchr::memchr(b'\n', body).unwrap_or(body.len());
                        if first_attached[i] == NONE {
                            first_attached[i] = comments.len() as u32;
                        }
                        comments.push(Comment {
                            range: (pos as u32)..(end as u32),
                            kind: CommentKind::Line,
                            token: i as u32,
                        });
                        pos = end;
                    }
                    b'/' if bytes.get(pos + 1) == Some(&b'*') => {
                        // Nested block comments, depth counted (scanner
                        // contract). An unterminated block comment (scanner
                        // error already reported) runs to the gap end.
                        let start = pos;
                        let mut depth = 1usize;
                        pos += 2;
                        while pos < gap_end && depth > 0 {
                            if bytes[pos] == b'/' && bytes.get(pos + 1) == Some(&b'*') {
                                depth += 1;
                                pos += 2;
                            } else if bytes[pos] == b'*' && bytes.get(pos + 1) == Some(&b'/') {
                                depth -= 1;
                                pos += 2;
                            } else {
                                pos += 1;
                            }
                        }
                        if first_attached[i] == NONE {
                            first_attached[i] = comments.len() as u32;
                        }
                        comments.push(Comment {
                            range: (start as u32)..(pos as u32),
                            kind: CommentKind::Block,
                            token: i as u32,
                        });
                    }
                    _ => pos += 1,
                }
            }
            cursor = gap_end + token.byte_len as usize;
        }

        Self {
            comments,
            first_attached,
        }
    }

    /// Whether any comment is attached to `token` (the official
    /// `token.precedingComments != null`).
    #[must_use]
    pub fn has_attached(&self, token: usize) -> bool {
        self.first_attached.get(token).is_some_and(|&c| c != NONE)
    }

    /// The comments attached to `token` (contiguous slice, empty when none).
    #[must_use]
    pub fn attached(&self, token: usize) -> &[Comment] {
        match self.first_attached.get(token) {
            Some(&first) if first != NONE => {
                let start = first as usize;
                let mut end = start + 1;
                while end < self.comments.len() && self.comments[end].token as usize == token {
                    end += 1;
                }
                &self.comments[start..end]
            }
            _ => &[],
        }
    }

    /// All comments in source order.
    #[must_use]
    pub fn all(&self) -> &[Comment] {
        &self.comments
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::Fixture;

    fn comments_of(source: &str) -> (Comments, usize) {
        let f = Fixture::build(source, |_, _| {});
        let count = f.tokens.len();
        (Comments::extract(&f.source, &f.tokens), count)
    }

    #[test]
    fn line_comment_attaches_to_following_token() {
        // Official anchor: comments precede a token (`precedingComments`),
        // `ignore_info.dart:479-497`.
        let (comments, _) = comments_of("var x = 1; // trailing\nvar y = 2;");
        let all = comments.all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].kind, CommentKind::Line);
        // Attached to the token after the gap: the `var` of line 2 (index 5).
        assert_eq!(all[0].token, 5);
        assert!(comments.has_attached(5));
        assert!(!comments.has_attached(4));
        assert_eq!(&comments.all()[0].range, &(11..22));
    }

    #[test]
    fn leading_and_block_comments() {
        let (comments, _) = comments_of("/* lead */ var x;\n// own line\nvar y;");
        let all = comments.all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].kind, CommentKind::Block);
        assert_eq!(all[0].token, 0); // `var` of `var x;` — the file's first token
        assert_eq!(all[1].kind, CommentKind::Line);
        assert_eq!(all[1].token, 3); // `var` of `var y;`
    }

    #[test]
    fn nested_block_comments() {
        let (comments, _) = comments_of("/* a /* b */ still */ var x;");
        let all = comments.all();
        assert_eq!(all.len(), 1, "nested block comment is one comment");
        assert_eq!(all[0].kind, CommentKind::Block);
    }

    #[test]
    fn slash_marks_inside_strings_are_not_comments() {
        // A `//` inside a string literal lies inside one token's range and
        // never in a gap.
        let (comments, _) = comments_of(r#"var s = '// ignore: x'; var t = "/* not */";"#);
        assert!(comments.all().is_empty());
    }

    #[test]
    fn multiple_comments_chain_on_one_token() {
        let (comments, _) = comments_of("// a\n// b\nvar x;");
        let all = comments.all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].token, 0); // both precede the file's first token
        assert_eq!(all[1].token, 0);
        assert_eq!(comments.attached(0).len(), 2);
    }
}
