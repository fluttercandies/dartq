//! Suppression comments: the `// ignore:` / `// ignore_for_file:` family,
//! ported from the analyzer's ignore-comment machinery
//! (`pkg/analyzer/lib/src/ignore_comments/ignore_info.dart`, Dart 3.15 main).
//!
//! Behavior anchors mirrored here:
//! - Which comments qualify: [`is_ignore_comment`] / [`is_ignore_for_file_comment`]
//!   (`ignore_info.dart:217-289`) — `ignore:` tolerates extra leading slashes,
//!   `ignore_for_file:` requires exactly two (that loop is commented out in
//!   the official source and stays out here).
//! - Line attribution (`IgnoreInfo.forDart`, ignore_info.dart:118-141): a
//!   comment alone on its line suppresses the NEXT line; a trailing comment
//!   suppresses its own line.
//! - Element parsing (`CommentTokenExtension.ignoredElements`,
//!   ignore_info.dart:292-472): a comma-separated list of diagnostic names,
//!   `type=<diagnosticType>` entries, and `plugin/name` prefixed names; the
//!   first character that does not fit terminates the list — trailing prose
//!   is never a name (ignore_info.dart:372-386, 424-438).
//! - Name matching (`IgnoredDiagnosticName._matches`, ignore_info.dart:39-52):
//!   case-insensitive against the code name. `plugin/name` entries match only
//!   their plugin; wave 1 has no plugins, so they match nothing.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::comments::Comments;

/// What one suppression entry matches.
#[derive(Debug)]
enum Entry {
    /// A diagnostic name; matches case-insensitively. The byte span is the
    /// name token's source-absolute range — the `unignorable_ignore` row's
    /// anchor (`IgnoreValidator`, ignore_validator.dart:170: offset of the
    /// ignored element, length of the name).
    Name { name: String, start: usize },
    /// A `plugin/name` entry; wave 1 has no plugins, matches nothing
    /// (ignore_info.dart:40-42 requires pluginName equality).
    PluginName,
    /// A `type=x` entry; covers lints when `x` is `lint`
    /// (`IgnoredDiagnosticType._matches`, ignore_info.dart:67-76).
    Type(String),
}

impl Entry {
    fn matches(&self, name: &str) -> bool {
        match self {
            Entry::Name { name: entry, .. } => entry.eq_ignore_ascii_case(name),
            Entry::PluginName => false,
            Entry::Type(t) => t.eq_ignore_ascii_case("lint"),
        }
    }
}

/// One file's suppression table: per-line entries plus whole-file entries.
#[derive(Debug, Default)]
pub struct IgnoreInfo {
    /// 1-based line number -> entries (`_ignoredOnLine`).
    on_line: FxHashMap<u32, Vec<Entry>>,
    /// File-wide entries (`_ignoredForFile`).
    for_file: Vec<Entry>,
}

impl IgnoreInfo {
    /// Builds the table from the extracted comments of one unit
    /// (`CompilationUnitExtension.ignoreComments` collecting, then
    /// `IgnoreInfo.forDart` splitting — ignore_info.dart:476-500, 118-141).
    #[must_use]
    pub fn parse(source: &str, comments: &Comments) -> Self {
        let mut info = Self::default();
        for comment in comments.all() {
            let lexeme = &source[comment.range.start as usize..comment.range.end as usize];
            if !(is_ignore_comment(lexeme) || is_ignore_for_file_comment(lexeme)) {
                continue;
            }
            if lexeme.contains("ignore:") {
                // Own line (only whitespace before the `//`) -> next line
                // (ignore_info.dart:126-133); trailing -> same line.
                let line = line_of(source, comment.range.start);
                let alone = source
                    [line_start_of(source, comment.range.start)..comment.range.start as usize]
                    .trim()
                    .is_empty();
                let target = if alone { line + 1 } else { line };
                info.on_line
                    .entry(target)
                    .or_default()
                    .extend(elements_of(lexeme, comment.range.start as usize));
            } else if lexeme.contains("ignore_for_file:") {
                info.for_file
                    .extend(elements_of(lexeme, comment.range.start as usize));
            }
        }
        info
    }

    /// Whether a lint named `name` reported on 1-based `line` is suppressed
    /// (`IgnoreInfo._ignoredAt`, ignore_info.dart:192-212: file-wide first,
    /// then the report's line).
    #[must_use]
    pub fn ignored(&self, line: u32, name: &str) -> bool {
        self.for_file.iter().any(|e| e.matches(name))
            || self
                .on_line
                .get(&line)
                .is_some_and(|entries| entries.iter().any(|e| e.matches(name)))
    }
}

/// `IgnoreInfo.isIgnoreComment` (ignore_info.dart:217-246): `//`, any number
/// of extra slashes, spaces, then `ignore:`.
fn is_ignore_comment(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 9 || !bytes.starts_with(b"//") {
        return false;
    }
    let mut from = 2;
    while from < bytes.len() && bytes[from] == b'/' {
        from += 1;
    }
    while from < bytes.len() && bytes[from] == b' ' {
        from += 1;
    }
    bytes[from..].starts_with(b"ignore:")
}

/// `IgnoreInfo.isIgnoreForFileComment` (ignore_info.dart:251-289): exactly
/// two slashes, spaces, then `ignore_for_file:`.
fn is_ignore_for_file_comment(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 18 || !bytes.starts_with(b"//") {
        return false;
    }
    let mut from = 2;
    while from < bytes.len() && bytes[from] == b' ' {
        from += 1;
    }
    bytes[from..].starts_with(b"ignore_for_file:")
}

/// `CommentTokenExtension.ignoredElements` (ignore_info.dart:295-471):
/// parse the list after the first `:` of the lexeme. `base` is the lexeme's
/// source-absolute byte offset — the name spans it produces are absolute.
fn elements_of(lexeme: &str, base: usize) -> Vec<Entry> {
    match lexeme.find(':') {
        Some(colon) => parse_element_list(&lexeme[colon + 1..], base + colon + 1),
        None => Vec::new(),
    }
}

/// The word/`type=`/`plugin/name` list parser, mirroring
/// `ignoredElements`'s `skipPastWhitespace`/`readWord` structure
/// (ignore_info.dart:299-321). `base` is the list text's source-absolute
/// byte offset.
fn parse_element_list(rest: &str, base: usize) -> Vec<Entry> {
    let mut entries = Vec::new();
    let bytes = rest.as_bytes();
    let n = bytes.len();
    let mut offset = 0usize;

    let skip_ws = |mut o: usize| {
        while o < n && bytes[o].is_ascii_whitespace() {
            o += 1;
        }
        o
    };
    // readWord: starts with a letter or underscore, continues with letters,
    // digits, underscores (ignore_info.dart:308-321). Returns the end, or
    // `o` itself when no word starts there.
    let read_word = |o: usize| -> usize {
        if o >= n || !(bytes[o].is_ascii_alphabetic() || bytes[o] == b'_') {
            return o;
        }
        let mut e = o + 1;
        while e < n && (bytes[e].is_ascii_alphanumeric() || bytes[e] == b'_') {
            e += 1;
        }
        e
    };
    // A parsed entry is complete only when the list continues correctly:
    // end, whitespace, or a comma (ignore_info.dart:372-386, 424-438 —
    // trailing garbage such as `ignore: http://x` yields no entry).
    let list_continues = |o: usize| o == n || bytes[o].is_ascii_whitespace() || bytes[o] == b',';

    loop {
        offset = skip_ws(offset);
        if offset == n {
            return entries;
        }
        let word_start = offset;
        let word_end = read_word(offset);
        if word_end == offset {
            // Non-word junk where a name should start.
            return entries;
        }
        let word = &rest[word_start..word_end];
        offset = word_end;
        if word.eq_ignore_ascii_case("type") {
            offset = skip_ws(offset);
            if offset == n || bytes[offset] != b'=' {
                return entries;
            }
            offset = skip_ws(offset + 1);
            if offset == n {
                return entries;
            }
            let type_start = offset;
            let type_end = read_word(offset);
            if type_end == offset || !list_continues(type_end) {
                return entries;
            }
            offset = type_end;
            entries.push(Entry::Type(rest[type_start..type_end].to_owned()));
        } else if offset < n && bytes[offset] == b'/' {
            // `plugin/name` (ignore_info.dart:398-423).
            offset = skip_ws(offset + 1);
            let name_end = read_word(offset);
            if name_end == offset || !list_continues(name_end) {
                return entries;
            }
            entries.push(Entry::PluginName);
            offset = name_end;
        } else {
            if !list_continues(offset) {
                return entries;
            }
            entries.push(Entry::Name {
                name: word.to_owned(),
                start: base + word_start,
            });
        }
        if offset == n {
            return entries;
        }
        offset = skip_ws(offset);
        if offset == n || bytes[offset] != b',' {
            // Trailing prose: unstructured comment text — done
            // (ignore_info.dart:454-466).
            return entries;
        }
        offset += 1;
    }
}

/// One `unignorable_ignore` hit: the ignore-comment name the options'
/// `cannot-ignore` set claims, with its UTF-16 anchor
/// (`IgnoreValidator._reportUnignorableAndDuplicateIgnores`,
/// ignore_validator.dart:164-174 — the row sits on the name token, and the
/// name leaves the suppression list so it can never suppress).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnignorableIgnore {
    /// The name as written in the comment (the message's `{0}`).
    pub name: String,
    /// The name token's UTF-16 offset.
    pub offset: u32,
    /// The name token's UTF-16 length.
    pub length: u32,
}

/// The `IgnoreValidator` face over one file's comments (dart 3.15 main,
/// ignore_validator.dart:73-131): every ignore-comment *name* — file-level
/// first in comment order, then line-level in line order — that the options'
/// `cannot-ignore` set contains reports once. The membership test is the
/// official case-sensitive `.contains(name)` against the lowercased set
/// (ignore_validator.dart:89, 113; the assert at :69 pins the set
/// lowercase), so a differently-cased spelling never hits. Suppression
/// itself is the [`IgnoreInfo`] consumer's gate (`run`'s `unignorable`
/// parameter) — the validator only reports.
///
/// Empty `unignorable` returns immediately: the dominant workspace pays
/// nothing.
#[must_use]
pub fn unignorable_ignores(
    source: &str,
    unignorable: &FxHashSet<String>,
) -> Vec<UnignorableIgnore> {
    if unignorable.is_empty() {
        return Vec::new();
    }
    // The comments come from the scan alone — no parse; the face runs under
    // the lint stage's per-file job.
    let mut files = dartq_source::FileTable::default();
    let file = files.intern(std::path::Path::new("unit.dart"));
    let scanned = crate::bridge::scan_tokens(source, file);
    let comments = Comments::extract(source, &scanned);
    let info = IgnoreInfo::parse(source, &comments);
    let mut hits = Vec::new();
    let mut push = |entry: &Entry| {
        if let Entry::Name { name, start, .. } = entry
            && unignorable.contains(name)
        {
            hits.push(UnignorableIgnore {
                name: name.clone(),
                offset: dartq_source::byte_offset_to_utf16(source, *start),
                length: dartq_source::utf16_len_of(name),
            });
        }
    };
    for entry in &info.for_file {
        push(entry);
    }
    // Dart's map iterates in insertion order (first-seen line); the Rust
    // table sorts by line for the same deterministic order.
    let mut lines: Vec<u32> = info.on_line.keys().copied().collect();
    lines.sort_unstable();
    for line in lines {
        for entry in &info.on_line[&line] {
            push(entry);
        }
    }
    hits
}

/// 1-based line number of a byte offset.
#[expect(
    clippy::cast_possible_truncation,
    clippy::naive_bytecount,
    reason = "line numbers fit u32 by the SourceRange convention; the plain \
              byte count avoids a new dependency on this cold path"
)]
fn line_of(source: &str, byte_offset: u32) -> u32 {
    1 + source.as_bytes()[..byte_offset as usize]
        .iter()
        .filter(|&&b| b == b'\n')
        .count() as u32
}

/// Byte offset of the start of the line containing `byte_offset`.
fn line_start_of(source: &str, byte_offset: u32) -> usize {
    source.as_bytes()[..byte_offset as usize]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(lexeme: &'static str) -> Vec<Entry> {
        elements_of(lexeme, 0)
    }

    fn names(lexeme: &'static str) -> Vec<String> {
        // Names only, for readability of the assertions below.
        parse(lexeme)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Name { name, .. } => Some(name),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ignore_comment_shapes() {
        // ignore_info.dart:217-246 — extra slashes and spaces tolerated.
        assert!(is_ignore_comment("// ignore: foo"));
        assert!(is_ignore_comment("///ignore:foo"));
        assert!(is_ignore_comment("//   ignore: foo"));
        assert!(!is_ignore_comment("// ignore_for_file: foo"));
        assert!(!is_ignore_comment("/* ignore: foo */"));
        assert!(!is_ignore_comment("//iggy: foo"));
    }

    #[test]
    fn ignore_for_file_shapes() {
        // ignore_info.dart:251-289 — exactly two slashes.
        assert!(is_ignore_for_file_comment("// ignore_for_file: foo"));
        assert!(!is_ignore_for_file_comment("/// ignore_for_file: foo"));
        assert!(!is_ignore_for_file_comment("// ignore: foo"));
    }

    #[test]
    fn element_list_parsing() {
        assert_eq!(names("// ignore: a,b"), ["a", "b"]);
        assert_eq!(names("// ignore: a , b"), ["a", "b"]);
        assert_eq!(names("// ignore: a // trailing"), ["a"]);
        assert_eq!(names("// ignore: http://x"), Vec::<String>::new());
        assert_eq!(names("// ignore: type=lint").len(), 0); // Type entry, not Name
        assert!(matches!(parse("// ignore: type=lint")[0], Entry::Type(_)));
        assert!(matches!(parse("// ignore: p/q")[0], Entry::PluginName));
    }

    #[test]
    fn matching_is_case_insensitive_name_only() {
        let info = crate::IgnoreInfo {
            on_line: Default::default(),
            for_file: vec![Entry::Name {
                name: "Empty_Statements".to_owned(),
                start: 0,
            }],
        };
        assert!(info.ignored(1, "empty_statements"));
        assert!(!info.ignored(1, "avoid_empty_else"));
    }

    #[test]
    fn type_lint_matches_all_lints() {
        let info = crate::IgnoreInfo {
            on_line: Default::default(),
            for_file: vec![Entry::Type("lint".to_owned())],
        };
        assert!(info.ignored(7, "empty_statements"));
        let info = crate::IgnoreInfo {
            on_line: Default::default(),
            for_file: vec![Entry::Type("hint".to_owned())],
        };
        assert!(!info.ignored(7, "empty_statements"));
    }

    #[test]
    fn unignorable_hits_pin_names_with_spans() {
        use crate::testsupport::Fixture;
        let set = rustc_hash::FxHashSet::from_iter(["empty_statements".to_owned()]);
        // The hit names the comment's name token: trailing `// ignore: a,
        // empty_statements` — the name starts after the comment head.
        let f = Fixture::build("var x = 1; // ignore: a, empty_statements\n", |_, _| {});
        let hits = unignorable_ignores(&f.source, &set);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].name, "empty_statements");
        // `var x = 1; // ignore: a, ` = 25 UTF-16 units.
        assert_eq!(hits[0].offset, 25);
        assert_eq!(hits[0].length, 16);

        // File-level entries report first, then per-line in line order.
        let f = Fixture::build(
            "// ignore_for_file: empty_statements\n// ignore: empty_statements\nvar x = 1;\n",
            |_, _| {},
        );
        let hits = unignorable_ignores(&f.source, &set);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].offset, 20, "the for_file name token");
        assert_eq!(hits[1].offset, 48, "the line entry's name token");
    }

    #[test]
    fn unignorable_matching_is_case_sensitive_on_comment_names() {
        use crate::testsupport::Fixture;
        // The official set is lowercased (assert at ignore_validator.dart:69)
        // and the comment name matches verbatim (:89, :113) — a
        // differently-cased spelling never hits.
        let set = rustc_hash::FxHashSet::from_iter(["empty_statements".to_owned()]);
        let f = Fixture::build("// ignore: Empty_Statements\nvar x = 1;\n", |_, _| {});
        assert!(unignorable_ignores(&f.source, &set).is_empty());

        // Non-member names and the empty set report nothing.
        let f = Fixture::build("// ignore: avoid_print\nvar x = 1;\n", |_, _| {});
        assert!(unignorable_ignores(&f.source, &set).is_empty());
        assert!(unignorable_ignores(&f.source, &rustc_hash::FxHashSet::default()).is_empty());
    }

    #[test]
    fn unignorable_spans_count_utf16_units() {
        use crate::testsupport::Fixture;
        let set = rustc_hash::FxHashSet::from_iter(["empty_statements".to_owned()]);
        // `\u{1F600}` is one UTF-16 unit pair... two units (a surrogate
        // pair) over four UTF-8 bytes: the name offset counts units, not
        // bytes.
        let f = Fixture::build("// \u{1F600}\n// ignore: empty_statements\n", |_, _| {});
        let hits = unignorable_ignores(&f.source, &set);
        assert_eq!(hits.len(), 1, "{hits:?}");
        // Line 1 = `// ` + 2 units + `\n` = 6 units; `// ignore: ` = 11.
        assert_eq!(hits[0].offset, 6 + 11);
    }

    #[test]
    fn line_attribution_own_line_vs_trailing() {
        use crate::comments::Comments;
        use crate::testsupport::Fixture;
        // Own line -> next line (ignore_info.dart:126-133).
        let f = Fixture::build("// ignore: empty_statements\nvar x = 1;", |_, _| {});
        let comments = Comments::extract(&f.source, &f.tokens);
        let info = IgnoreInfo::parse(&f.source, &comments);
        assert!(info.ignored(2, "empty_statements"));
        assert!(!info.ignored(1, "empty_statements"));
        // Trailing -> same line.
        let f = Fixture::build("var x = 1; // ignore: empty_statements\n", |_, _| {});
        let comments = Comments::extract(&f.source, &f.tokens);
        let info = IgnoreInfo::parse(&f.source, &comments);
        assert!(info.ignored(1, "empty_statements"));
        assert!(!info.ignored(2, "empty_statements"));
    }
}
