//! Golden smoke: every scanner-golden corpus case (74 cases from the official
#![allow(clippy::cast_possible_truncation)] // UTF-16 lengths are u32 by design
//! SDK scanner tests) must flow through the parser without panics, reaching
//! EOF with in-bounds diagnostics.
//!
//! The corpus is scanner-shaped (operator matrices, string/interpolation
//! samples, recovery snippets) — not whole programs — so this smoke asserts
//! parser *robustness* rather than diagnostic parity:
//!
//! 1. `parse_unit` (full declaration dispatch since #18) consumes the whole
//!    stream and reaches EOF, with every parser diagnostic inside the
//!    source's UTF-16 bounds except the upstream EOF face (start == len,
//!    end == len + 1 — the oracle's EOF-anchored length-1 row);
//! 2. at every token position, speculative type, pattern, and top-level
//!    declaration parses either advance the cursor or leave it unchanged
//!    (no hangs), never panic;
//! 3. event/`TokenSpan` invariants hold for every emitted diagnostic: UTF-16
//!    offsets within `[0, utf16_len]`, plus the EOF face.
//!
//! When #19/#20 fill in statements and the full expression engine, these
//! same drivers graduate into differential coverage over the corpus.

use std::path::PathBuf;

use super::collector::{Collector, latest_features};
use crate::listener::{NullListener, PatternContext};
use crate::parser::Parser;

const MANIFEST: &str = "tests/corpus/scanner-goldens/manifest.json";

struct Case {
    source_file: String,
    triple_shift: bool,
    augment: bool,
}

fn manifest_cases() -> Vec<Case> {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(MANIFEST);
    let raw = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    parse_manifest(&raw)
}

/// Minimal JSON reader for the manifest: enough for the flat case fields we
/// consume (id/source paths + config booleans). Avoids a serde dependency
/// for one test file.
fn parse_manifest(raw: &str) -> Vec<Case> {
    // The manifest is pretty-printed with one field per line; the config
    // block for each case follows its expectation field. Case boundaries are
    // the `id` lines.
    let mut cases = Vec::new();
    for block in raw.split("\"id\"").skip(1) {
        let Some(source_start) = block.find("\"source\": \"") else {
            continue;
        };
        let after = &block[source_start + "\"source\": \"".len()..];
        let source_file = after[..after.find('"').expect("quoted source path")].to_owned();
        let triple_shift = block.contains("\"triple_shift\": true");
        let augment = block.contains("\"augment\": true");
        cases.push(Case {
            source_file,
            triple_shift,
            augment,
        });
    }
    cases
}

/// Loads a case source: strips the 2-line provenance header (the corpus
/// loading contract) and returns the remainder.
fn load_case_source(case: &Case) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join("tests/corpus/scanner-goldens")
        .join(&case.source_file);
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let without_header = raw.splitn(3, '\n').nth(2).unwrap_or_default();
    without_header.to_owned()
}

fn scan_case(source: &str, case: &Case) -> dartq_scanner::ScanResult {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: case.triple_shift,
        enable_augmentations: case.augment,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("case.dart"));
    dartq_scanner::scan(source, config, file)
}

#[test]
fn all_golden_cases_consume_cleanly() {
    let cases = manifest_cases();
    assert_eq!(cases.len(), 74, "corpus case count changed");
    for case in &cases {
        let source = load_case_source(case);
        let result = scan_case(&source, case);
        let utf16_len: u32 = source.chars().map(char::len_utf16).sum::<usize>() as u32;

        // 1. Declaration-dispatch unit parse: total consumption, and every
        //    parser diagnostic stays inside the source.
        let mut listener = Collector::default();
        {
            let mut parser = Parser::new(&source, &result.tokens, latest_features(), &mut listener);
            parser.parse_unit();
        }
        for (code, start, end) in &listener.errors {
            // Upstream reports EOF-anchored diagnostics AT the EOF token:
            // findNonZeroLengthToken stops its walk at EOF
            // (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/parser/util.dart:56),
            // and the oracle machine rows pin the length-1 EOF face
            // (`'}' @2:1|1`, offset == len) — admit exactly start == len,
            // end == len + 1, reject anything else out of bounds.
            let eof_anchored = *start == utf16_len && *end == utf16_len + 1;
            assert!(
                eof_anchored || (*start <= utf16_len && *end <= utf16_len),
                "case {}: parser diagnostic {code} out of bounds ({start}..{end}, utf16 len {utf16_len})",
                case.source_file
            );
        }

        // 2. Speculative type/pattern/declaration parses at every position:
        //    no panics, no hangs, cursor stays in bounds.
        for start in 0..result.tokens.len() {
            for kind in 0..3 {
                let mut null = NullListener;
                let mut parser = Parser::new(&source, &result.tokens, latest_features(), &mut null);
                for _ in 0..start {
                    parser.cursor.advance();
                }
                let before = parser.cursor.real_pos();
                match kind {
                    0 => {
                        let _ = parser.parse_type_or_void_entry();
                    }
                    1 => {
                        let _ = parser.parse_pattern_entry(PatternContext::Matching);
                    }
                    _ => {
                        parser.parse_top_level_declaration();
                    }
                }
                let after = parser.cursor.real_pos();
                assert!(
                    after >= before && after < result.tokens.len(),
                    "case {} pos {start} kind {kind}: cursor escaped bounds ({before} -> {after})",
                    case.source_file
                );
            }
        }

        // 3. Scanner diagnostics stay in UTF-16 bounds (parser shares the
        //    convention through TokenSpan).
        for diag in &result.diagnostics {
            assert!(
                diag.range.offset <= utf16_len && diag.range.end() <= utf16_len,
                "case {}: diagnostic {:?} out of bounds (utf16 len {utf16_len})",
                case.source_file,
                diag.code.name
            );
        }
    }
}
