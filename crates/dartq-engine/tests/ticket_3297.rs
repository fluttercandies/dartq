//! Ticket #3297 (residual): the enhanced-parts **combined import scope**.
//!
//! The rule (`parts-with-imports`, co19 `LanguageFeatures/Parts-with-imports`):
//! "Each Dart file (library file or part file) defines a combined import scope
//! which extends the scope chain of the parent file's combined import scope
//! with its own imports and import prefixes. It is this combined import scope
//! that sub-parts inherit and override." The top-level **declaration**
//! namespace stays shared across the library's files — only the imports and
//! prefixes are per file.
//!
//! The two shapes below are the co19 `scope_A02_t04` (combinators across the
//! part boundary) and `scope_A07_t01` (per-part imports and a per-part `as`
//! prefix) cases, reduced to their resolution face; the expected rows are the
//! oracle's (`dart analyze` 3.13.7 with `--enable-experiment=enhanced-parts`).
//!
//! Before the fix the resolution graph merged every member file's imports
//! into one flat per-library namespace, so a name imported by one part
//! resolved in every sibling part and in the library file — every expected row
//! below was missing (an `undefined_identifier` false negative).

mod common;

use common::Fixture;
use dartq_engine::{CancellationToken, Engine};

/// The `undefined_identifier` rows of a run as `(file tail, undefined name)`,
/// sorted — the file/name pair is the conformance face (the message is
/// `Undefined name '<name>'.`).
fn undefined_names(
    engine: &Engine,
    diagnostics: &[dartq_diagnostics::Diagnostic],
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = diagnostics
        .iter()
        .filter(|row| row.code.name == "undefined_identifier")
        .map(|row| {
            let file = engine
                .file_table()
                .path(row.file)
                .map(|path| {
                    path.to_string_lossy()
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                })
                .unwrap_or_default();
            let message = row.message.to_string();
            let name = message
                .strip_prefix("Undefined name '")
                .and_then(|rest| rest.strip_suffix("'."))
                .unwrap_or(&message)
                .to_owned();
            (file, name)
        })
        .collect();
    rows.sort();
    rows
}

/// Runs the whole fixture and returns its `undefined_identifier` rows.
fn analyze(fixture: &Fixture, files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut engine = fixture.engine();
    for (relative, content) in files {
        engine.set_content(&fixture.path(relative), content.as_bytes());
    }
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    undefined_names(&engine, &diagnostics)
}

/// `scope_A02_t04`: `show`/`hide` filter per import and per part. `part1`
/// imports `lib1.dart show LibClass` and `lib2.dart hide libId`; `part2` (a
/// sub-part) imports `lib2.dart show libId`.
///
/// * `part1` sees `LibClass` (shown) and `lib2Func` (not hidden), not
///   `libVar` (filtered by `show LibClass`) and not `libId` (hidden);
/// * `part2` sees its own `libId` and inherits `lib2Func` from `part1`;
/// * the library file imports nothing, so it sees neither.
#[test]
fn scope_a02_t04_filters_per_import_across_the_part_boundary() {
    const A: &str = "part 'part1.dart';\n\
                      void main() {\n\
                      \x20 testPart1();\n\
                      \x20 testPart2();\n\
                      }\n";
    const PART1: &str = "part of 'a.dart';\n\
                         import 'lib1.dart' show LibClass;\n\
                         import 'lib2.dart' hide libId;\n\
                         part 'part2.dart';\n\
                         void testPart1() {\n\
                         \x20 print(libVar);\n\
                         \x20 print(LibClass);\n\
                         \x20 print(libId);\n\
                         \x20 lib2Func();\n\
                         }\n";
    const PART2: &str = "part of 'part1.dart';\n\
                         import 'lib2.dart' show libId;\n\
                         void testPart2() {\n\
                         \x20 print(libId);\n\
                         \x20 lib2Func();\n\
                         }\n";
    const LIB1: &str = "int libVar = 0;\nclass LibClass {}\n";
    const LIB2: &str = "int libId = 0;\nvoid lib2Func() {}\n";

    let file_set = [
        ("lib/a.dart", A),
        ("lib/part1.dart", PART1),
        ("lib/part2.dart", PART2),
        ("lib/lib1.dart", LIB1),
        ("lib/lib2.dart", LIB2),
    ];
    let fixture = Fixture::with_enhanced_parts(&file_set);
    assert_eq!(
        analyze(&fixture, &file_set),
        vec![
            ("part1.dart".to_owned(), "libId".to_owned()),
            ("part1.dart".to_owned(), "libVar".to_owned()),
        ],
        "`show LibClass` and `hide libId` filter part1's imports; part2's own \
         `show libId` and its inherited `lib2Func` resolve"
    );
}

/// `scope_A07_t01`: imports and an `as` prefix written in a part belong to
/// that part and its sub-parts — never to the library file, never to a
/// sibling.
///
/// * the library file reports `libVar`, `l1` and `counter`: all three come
///   from `part1`/`part2` imports;
/// * `part1` reports `counter` only — its own bare import and prefix resolve,
///   `part2`'s import does not reach upward;
/// * `part2` reports nothing: it inherits `part1`'s scope and adds
///   `parts_lib.dart`.
#[test]
fn scope_a07_t01_part_imports_do_not_leak_up_or_sideways() {
    const A: &str = "part 'part1.dart';\n\
                      void main() {\n\
                      \x20 print(libVar);\n\
                      \x20 print(l1.lib2Func());\n\
                      \x20 print(counter);\n\
                      \x20 testPart1();\n\
                      \x20 testPart2();\n\
                      }\n";
    const PART1: &str = "part of 'a.dart';\n\
                         import 'lib1.dart';\n\
                         import 'lib2.dart' as l1;\n\
                         part 'part2.dart';\n\
                         void testPart1() {\n\
                         \x20 print(libVar);\n\
                         \x20 print(l1.lib2Func());\n\
                         \x20 print(counter);\n\
                         }\n";
    const PART2: &str = "part of 'part1.dart';\n\
                         import 'parts_lib.dart';\n\
                         void testPart2() {\n\
                         \x20 print(libVar);\n\
                         \x20 print(l1.lib2Func());\n\
                         \x20 print(counter);\n\
                         }\n";
    const LIB1: &str = "int libVar = 0;\n";
    const LIB2: &str = "void lib2Func() {}\n";
    const PARTS_LIB: &str = "int counter = 0;\n";

    let file_set = [
        ("lib/a.dart", A),
        ("lib/part1.dart", PART1),
        ("lib/part2.dart", PART2),
        ("lib/lib1.dart", LIB1),
        ("lib/lib2.dart", LIB2),
        ("lib/parts_lib.dart", PARTS_LIB),
    ];
    let fixture = Fixture::with_enhanced_parts(&file_set);
    assert_eq!(
        analyze(&fixture, &file_set),
        vec![
            ("a.dart".to_owned(), "counter".to_owned()),
            ("a.dart".to_owned(), "l1".to_owned()),
            ("a.dart".to_owned(), "libVar".to_owned()),
            ("part1.dart".to_owned(), "counter".to_owned()),
        ],
        "the chain runs upward only: the library file inherits no part's \
         scope, a part inherits its includer's"
    );
}

/// The classic shape stays intact: with the enhanced-parts grant in place but
/// no part file carrying directives, a part file still resolves the library
/// file's imports — its combined scope *is* its parent's, and the shared
/// top-level namespace carries both directions of declarations.
#[test]
fn classic_part_inherits_the_library_imports() {
    const A: &str = "import 'lib.dart';\npart 'p.dart';\nint shared = 1;\n";
    const P: &str = "part of 'a.dart';\nvoid use() {\n  print(libVar);\n  print(shared);\n}\n";
    const LIB: &str = "int libVar = 0;\n";

    let file_set = [("lib/a.dart", A), ("lib/p.dart", P), ("lib/lib.dart", LIB)];
    let fixture = Fixture::with_enhanced_parts(&file_set);
    assert_eq!(
        analyze(&fixture, &file_set),
        Vec::new(),
        "a directive-less part inherits the library file's scope"
    );
}
