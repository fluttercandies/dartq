//! Spanned pubspec facts for the lint face (`secure_pubspec_urls`,
//! `package_names`): the `name:` value and the URL-bearing values with
//! their byte spans, read by one linear scan over the raw text.
//!
//! Production callers run [`Pubspec::parse`][crate::Pubspec::parse] on the
//! same text first (the mounting pass needs the parsed dependency names
//! anyway), so well-formedness is already established upstream and this
//! scanner's tolerance face is deliberately narrow — a block-style mapping
//! walk, no error reporting.
//!
//! # Under-match faces (documented divergence)
//!
//! The official pubspec rules walk the parsed Yaml node tree, so they also
//! see shapes this scanner does not recognize. Where recognition fails the
//! value yields no row and stays silent (an under-report on valid YAML):
//!
//! - flow style (`dep: {git: {url: ...}}`, `dependencies: {...}`);
//! - quoted keys (`"homepage": ...`);
//! - block scalars (`homepage: >- ...`) and explicit empty values;
//! - anchors/aliases and merge keys.
//!
//! Quoted *values* are read (single-quoted `''` doubling, double-quoted
//! `\"`/`\\` escapes cooked). Any other double-quote escape (`\n`, `\t`,
//! `\u...`, ...) fails the scalar closed and yields no row: cooking
//! `\<char>` verbatim would misreport (a URL like `"\u0041.example"`
//! would read as `u0041.example`). Spans mirror the official
//! `YamlNode.span`: the raw scalar extent, quotes included.

use core::ops::Range;

/// One scalar fact: `range` is the raw source extent in the scanned text
/// (quotes included) and `value` the cooked scalar text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scalar {
    pub range: Range<usize>,
    pub value: String,
}

/// The lint-face fields of one pubspec text, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PubspecFacts {
    /// The top-level `name:` value.
    pub name: Option<Scalar>,
    /// The `homepage` / `repository` / `issue_tracker` / `documentation`
    /// values.
    pub urls: Vec<Scalar>,
    /// The `git`/`hosted` URLs of the three dependency sections — inline
    /// (`git: <url>`, `hosted: <url>`) or nested (`git: {url: ...}` block
    /// form), section order then entry order.
    pub dep_urls: Vec<Scalar>,
}

/// Scans the pubspec fields the lint face reports on. Linear in the text;
/// every line is visited once and nothing accumulates beyond the emitted
/// rows.
#[must_use]
pub fn scan(text: &str) -> PubspecFacts {
    let mut scan = Scan {
        facts: PubspecFacts::default(),
        section: false,
        section_indent: None,
        source_indent: None,
    };
    let mut offset = 0usize;
    for raw in text.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        scan.line(line, offset);
        offset += raw.len();
    }
    scan.facts
}

/// The per-line walk state. `section` tracks a top-level dependency
/// section; `section_indent` the dep-name indent inside it;
/// `source_indent` an open `git:`/`hosted:` block whose nested `url:` is
/// the dep's URL.
struct Scan {
    facts: PubspecFacts,
    section: bool,
    section_indent: Option<usize>,
    source_indent: Option<usize>,
}

impl Scan {
    fn line(&mut self, line: &str, offset: usize) {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let body = &line[indent..];
        // Blank and comment-only lines carry no keys.
        if body.is_empty() || body.starts_with('#') {
            return;
        }
        let Some(colon) = find_key_colon(body) else {
            return;
        };
        let key = body[..colon].trim();
        let (range, value) = scalar(&body[colon + 1..], offset + indent + colon + 1);

        if indent == 0 {
            // A dependency section opens only in block form; any other
            // top-level key closes the current one.
            self.section = matches!(
                key,
                "dependencies" | "dev_dependencies" | "dependency_overrides"
            ) && value.is_empty();
            self.section_indent = None;
            self.source_indent = None;
            if !self.section && !value.is_empty() {
                match key {
                    "name" => self.facts.name = Some(Scalar { range, value }),
                    "homepage" | "repository" | "issue_tracker" | "documentation" => {
                        self.facts.urls.push(Scalar { range, value });
                    }
                    _ => {}
                }
            }
            return;
        }
        if !self.section {
            return;
        }
        match self.section_indent {
            None => {
                // The first indented line fixes the dep-name indent.
                self.section_indent = Some(indent);
                self.source_indent = None;
            }
            Some(dep_indent) if indent <= dep_indent => {
                // The next dep entry (or the section's end): the previous
                // spec is over.
                self.source_indent = None;
            }
            Some(_) => {
                self.spec_line(indent, key, range, value);
            }
        }
    }

    /// One line inside the current dep's spec block (past `dep_indent`).
    fn spec_line(&mut self, indent: usize, key: &str, range: Range<usize>, value: String) {
        // A `url:` strictly under an open `git:`/`hosted:` block is the
        // dep's URL; a line at or above the block's indent closes it and
        // the line is re-read as a spec key below.
        let Some(open) = self.source_indent else {
            self.source_key(indent, key, range, value);
            return;
        };
        if indent <= open {
            self.source_indent = None;
            self.source_key(indent, key, range, value);
        } else if key == "url" && !value.is_empty() {
            self.facts.dep_urls.push(Scalar { range, value });
        }
    }

    /// A spec key with no open `git:`/`hosted:` block above it: the block
    /// forms open one, the inline forms carry the URL themselves.
    fn source_key(&mut self, indent: usize, key: &str, range: Range<usize>, value: String) {
        if !matches!(key, "git" | "hosted") {
            return;
        }
        if value.is_empty() {
            self.source_indent = Some(indent);
        } else {
            self.facts.dep_urls.push(Scalar { range, value });
        }
    }
}

/// The first `:` of `body` that terminates a key (followed by a space or
/// end of line — the `http://` colons inside values never are).
fn find_key_colon(body: &str) -> Option<usize> {
    let b = body.as_bytes();
    (0..b.len()).find(|&i| b[i] == b':' && (i + 1 == b.len() || b[i + 1] == b' '))
}

/// The inline scalar after `key:` (untrimmed, `abs` the offset just past
/// the colon): its absolute byte range — quotes included — and cooked
/// value. An empty value is block form; a `#` at the value head or after
/// whitespace ends a plain scalar.
fn scalar(rest: &str, abs: usize) -> (Range<usize>, String) {
    let trimmed = rest.trim_start();
    let abs = abs + (rest.len() - trimmed.len());
    let b = trimmed.as_bytes();
    match b.first() {
        None | Some(b'#') => (abs..abs, String::new()),
        Some(b'\'') => quoted(trimmed, abs, b'\''),
        Some(b'"') => quoted(trimmed, abs, b'"'),
        _ => {
            // A `#` after any whitespace ends a plain scalar (`a#b` has no
            // comment). The plain arm never sees a leading `#`, so every
            // match has a preceding byte.
            let end = trimmed
                .match_indices('#')
                .find(|&(i, _)| matches!(trimmed.as_bytes()[i - 1], b' ' | b'\t'))
                .map_or(trimmed.len(), |(i, _)| i);
            let value = trimmed[..end].trim_end();
            (abs..abs + value.len(), value.to_owned())
        }
    }
}

/// A quoted scalar: the raw extent spans the quotes; the cooked value is
/// the content with YAML's quote escapes resolved (single: `''` doubling;
/// double: only `\"` and `\\` — any other escape fails the scalar closed,
/// see the module doc). An unterminated literal is block form — upstream
/// parsing rejects it anyway.
fn quoted(trimmed: &str, abs: usize, quote: u8) -> (Range<usize>, String) {
    let b = trimmed.as_bytes();
    let mut value = String::new();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            // The escape consumes the closing candidate (double-quote form).
            b'\\' if quote == b'"' && i + 1 < b.len() => match b[i + 1] {
                b'"' | b'\\' => {
                    value.push(b[i + 1] as char);
                    i += 2;
                }
                // Unhandled escape: fail closed (no row for this scalar).
                _ => return (abs..abs, String::new()),
            },
            c if c == quote => {
                if quote == b'\'' && b.get(i + 1) == Some(&quote) {
                    value.push(quote as char);
                    i += 2;
                    continue;
                }
                return (abs..abs + i + 1, value);
            }
            _ => {
                // Multi-byte UTF-8: copy the whole char.
                let ch = trimmed[i..].chars().next().expect("char boundary");
                value.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    (abs..abs, String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(text: &str) -> Vec<String> {
        scan(text)
            .urls
            .into_iter()
            .map(|scalar| scalar.value)
            .collect()
    }

    #[test]
    fn double_quote_escapes_fail_closed() {
        // An unhandled escape emits no row for the scalar.
        assert_eq!(
            urls("homepage: \"http://a\\u0041.example\"\n"),
            Vec::<String>::new()
        );
        // The handled pair still cooks.
        assert_eq!(urls("homepage: \"a\\\\b\"\n"), vec!["a\\b"]);
        assert_eq!(urls("homepage: \"say \\\"hi\\\"\"\n"), vec!["say \"hi\""]);
        // Plain values are unaffected.
        assert_eq!(
            urls("homepage: http://a.example\n"),
            vec!["http://a.example"]
        );
    }

    #[test]
    fn plain_scalar_comment_ends_at_whitespace_hash() {
        assert_eq!(urls("homepage: url\t# c\n"), vec!["url"]);
        assert_eq!(urls("homepage: url # c\n"), vec!["url"]);
        // No whitespace before the `#`: part of the value.
        assert_eq!(urls("homepage: url#frag\n"), vec!["url#frag"]);
    }
}
