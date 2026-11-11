//! #3114 pins: the library / `part of` dotted-name walk stops at the
//! qualified-name boundary — `library lib1 library lib2` is two `library`
//! directives (`lib1` and `lib2`), never one name `lib1.library.lib2`.
//!
//! Upstream grammar is `qualified: identifier qualifiedRest*` with
//! `qualifiedRest: '.' identifier` (parser_impl.dart `parseQualified`
//! :2525-2535 / `parseQualifiedRest` :2558-2566), so a segment continues
//! the name only across a `.`. Both callers — `parseLibraryName`
//! (:907-925) and `parsePartOf` (:1315-1334) — parse the name through it
//! and end on `ensureSemicolon` (:5048-5063), which splices a synthetic
//! `;` in front of the follower; the top-level loop re-enters on that
//! follower, so the token after the qualified name starts the next
//! top-level declaration. Name position still takes built-in identifiers
//! (token.dart:794 — `isIdentifier` covers `isBuiltIn`), so
//! `library library;` names the library `library`.
//!
//! Library and `part of` facts carry no URI, so this face never reaches
//! the diagnostic oracle: the pins here walk the crate's own directive diet
//! harness (`l1::extract_directives` over a real scan) instead.

use std::path::Path;

use dartq_language::{DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet};
use dartq_parser::{Message, Parser, ParserFeatures, ParserListener, TokenSpan};
use dartq_scanner::{ScanConfig, scan};
use dartq_source::{FileTable, StringTable};

use crate::l1::extract_directives;
use crate::state::DirectiveKind;

/// Scans `text` once so both the directive-diet harness and the
/// recovery-diagnostics harness share the same token stream.
fn scan_text(text: &str) -> dartq_scanner::ScanResult {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("/lib1.dart"));
    let features = FeatureSet::for_language_version(DEFAULT_LANGUAGE_VERSION);
    scan(
        text,
        ScanConfig {
            enable_triple_shift: features.is_enabled(Feature::TripleShift),
            enable_augmentations: features.is_enabled(Feature::Augmentations),
        },
        file,
    )
}

/// The `(kind, name)` of every directive fact the diet walk yields for
/// `text`, in source order.
fn facts(text: &str) -> Vec<(DirectiveKind, Option<String>)> {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("/lib1.dart"));
    let scanned = scan_text(text);
    let strings = StringTable::new();
    let mut diagnostics = Vec::new();
    let (directives, _, _) =
        extract_directives(text, &scanned.tokens, &strings, file, &mut diagnostics);
    directives
        .into_iter()
        .map(|directive| (directive.kind, directive.name.as_deref().map(str::to_owned)))
        .collect()
}

/// The `(code, offset)` of every `missing_identifier` /
/// `multiple_library_directives` recovery row the real parser reports
/// for `text`, sorted for a stable comparison — the subset of the
/// oracle's `--format=machine` rows this pin asserts (offsets are
/// UTF-16, byte-identical for this ASCII fixture). `expected_token`
/// rows are deliberately excluded: they are semicolon-recovery noise
/// whose anchor convention is implementation detail, not the
/// synthetic-name semantics this pin guards.
fn recovery_diagnostics(text: &str) -> Vec<(&'static str, u32)> {
    const KEEP: [&str; 2] = ["missing_identifier", "multiple_library_directives"];
    struct Collector(Vec<(&'static str, u32)>);
    impl ParserListener for Collector {
        fn handle_recoverable_error(
            &mut self,
            message: Message,
            start: TokenSpan,
            _end: TokenSpan,
        ) {
            if KEEP.contains(&message.code.name) {
                self.0.push((message.code.name, start.range.offset));
            }
        }
    }
    let scanned = scan_text(text);
    let features = FeatureSet::for_language_version(DEFAULT_LANGUAGE_VERSION);
    let mut collector = Collector(Vec::new());
    Parser::new(
        text,
        &scanned.tokens,
        ParserFeatures::from_feature_set(features),
        &mut collector,
    )
    .parse_unit();
    collector.0.sort_unstable();
    collector.0
}

fn library(name: &str) -> (DirectiveKind, Option<String>) {
    (DirectiveKind::Library, Some(name.to_owned()))
}

/// `library lib1 library lib2;` — the `;`-less first directive ends in
/// front of the second head: two `library` facts, `lib1` and `lib2`.
#[test]
fn unterminated_library_keeps_the_following_library() {
    assert_eq!(
        facts("library lib1 library lib2;"),
        vec![library("lib1"), library("lib2")]
    );
}

/// The ticket's exact tail shape (no trailing `;`): the synthetic-`;`
/// boundary sits in front of the second head the same way.
#[test]
fn unterminated_library_keeps_the_following_library_without_a_semicolon() {
    assert_eq!(
        facts("library lib1 library lib2"),
        vec![library("lib1"), library("lib2")]
    );
}

/// A well-formed qualified name still joins across its periods, and the
/// written `;` closes it.
#[test]
fn a_qualified_name_joins_across_its_periods() {
    assert_eq!(facts("library a.b.c;"), vec![library("a.b.c")]);
}

/// A built-in identifier that reaches name position only to be followed by
/// another declaration head is NOT a name: `LibraryIdentifierContext.
/// ensureIdentifier` (upstream `identifier_context_impl.dart`:947-1000)
/// splices a synthetic name in front of it and hands it back to the
/// top-level loop, so the name slot stays empty and the head starts its own
/// `library` directive. The oracle agrees — `dart analyze --format=machine`
/// on this fixture yields three `library` directives (the two synthetic
/// empty names plus `fetch`) and reports `MISSING_IDENTIFIER` on each
/// would-be name `library` (cols 9 and 17 → offsets 8 and 16) plus
/// `MULTIPLE_LIBRARY_DIRECTIVES` on each repeat.
#[test]
fn a_builtin_identifier_before_a_directive_head_is_a_synthetic_name() {
    assert_eq!(
        facts("library library library fetch;"),
        vec![
            (DirectiveKind::Library, Some(String::new())),
            (DirectiveKind::Library, Some(String::new())),
            library("fetch"),
        ]
    );
    // The synthetic names surface as `missing_identifier` on each would-be
    // name `library` (offsets 8 and 16 = oracle cols 9 and 17) and the
    // three-directive file as `multiple_library_directives` on each repeat
    // `library` keyword (same two offsets).
    assert_eq!(
        recovery_diagnostics("library library library fetch;"),
        vec![
            ("missing_identifier", 8),
            ("missing_identifier", 16),
            ("multiple_library_directives", 8),
            ("multiple_library_directives", 16),
        ]
    );
}

/// A bare name token after the first segment is not a continuation — the
/// grammar continues only across a `.`, so the name stays `a`.
#[test]
fn a_name_token_without_a_period_does_not_continue_the_name() {
    assert_eq!(facts("library a b;"), vec![library("a")]);
}

/// `part of` shares the walk: `part of a library b;` yields the part-of
/// name `a` and the following `library` as its own directive
/// (`parsePartOf` ends on `ensureSemicolon` too).
#[test]
fn unterminated_part_of_keeps_the_following_library() {
    assert_eq!(
        facts("part of a library b;"),
        vec![(DirectiveKind::PartOf, Some("a".to_owned())), library("b"),]
    );
}
