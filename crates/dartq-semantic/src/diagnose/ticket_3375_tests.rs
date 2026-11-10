//! The #3375 pins — the C12 prefix/combinator resolution faces
//! (`import 'x.dart' show Foo hide Bar as p;` then `p.Foo`), oracle-anchored
//! against the recon's failing rows.
//!
//! # What the family actually is
//!
//! All 21 C12 rows (10 EXTRA / 11 MISSING) root OUTSIDE `library.rs`; the
//! combinator decision surfaces this crate owns — [`Combinators::allows`]
//! (dartq-resolution), the `Namespaces` bare/prefix probes with their clause
//! twins (namespace.rs `probe`/`allows`), and the library.rs consumption
//! faces (`LibraryScopes::build`, `lookup_scoped`, `import_shadows`,
//! `prefixed_container`) — already implement the upstream shape
//! (`named_type_resolver.dart:134-332` + `PrefixScope` composition,
//! scope.dart:352-357/530-535). The pins below prove those faces with the
//! capture the engine diet walk SHOULD produce, so the one-line capture fix
//! lands against pinned semantics instead of a blind edit.
//!
//! # Residuals (each root owned by another file; not fixed here)
//!
//! - EXTRA `show hide, show hide ugly` (combinators2_test 11:9|4 `hide`,
//!   12:9|4 `show`; combinators_test 19:25|4, 20:25|4, part 10:10|4): the
//!   engine diet walk's name-slot gate accepts only `Identifier` and
//!   built-in identifiers, so the PSEUDO keywords `show`/`hide` are dropped
//!   from the clause name lists and the `show` clause imports nothing —
//!   `dartq-engine/src/l1.rs:1990` (`is_name_token`) must admit
//!   `KeywordStyle::Pseudo` in the combinator name loop (`refuses_name`
//!   already owns the next-declaration-head boundary, and pseudo keywords
//!   never look like declaration heads). The parser itself is correct:
//!   `ensure_identifier` consumes pseudo keywords in `Combinator` context.
//! - EXTRA implicit-creation heads (`nonPrefix.Class<int>.named()` with a
//!   defined function head at 4:3; `C<E>.n()` at 2:8 where the oracle's own
//!   code is `creation_with_non_type`): the head-row code/span shapes live
//!   in the diagnose emission sites (held files); the same-position display
//!   twins (`unresolved` |16 vs |10, `prefix` |8 vs |6, `p` |3 vs |1) are
//!   the head span covering `.Class`/`<T>` instead of the identifier alone.
//! - MISSING unquoted-imports rows (keywords_A01_t02 22:44|3 `l08`,
//!   48:30|3; static_semantics_A02_t02 17:27|1 `c`, 18:33|1 `d`, 25:9,
//!   26:9, 29:9, 30:9, 31:9): the oracle at the 3.13.7 pin recovers the
//!   unquoted `import` directives as broken declarations and reports
//!   `undefined_class` on the `as` tail plus `undefined_identifier` on the
//!   never-registered prefixes, while dartq implements the feature (its own
//!   rows are `not_a_type`/`undefined_prefixed_name`). Matching requires
//!   the unquoted-imports feature posture decision (parser/feature level).
//! - MISSING prefixed-NamedType-in-pattern rows (extractor_
//!   prefixedNamedUnderscore_withTypeArgs_insideCase 3:10|8,
//!   object_prefixed_withTypeArgs_insideAssignment 2:3|12 `Future`): the
//!   oracle reports `undefined_class` naming the class segment over the
//!   full `_.Future` spelling when the prefix is undefined
//!   (`reportNullOrNonTypeElement`, `_getErrorRange` spanning
//!   prefix-through-name). dartq's `arm_type_prefix` implements exactly
//!   that shape for `PrefixedTypeReference` heads, but the object-pattern /
//!   extractor faces never route one (listener pattern-face routing).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringId, StringTable};
use std::path::Path;

/// The `undefined_identifier` rows of `report` in report order as
/// `(offset, length)`.
fn undefined_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_identifier")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The `undefined_prefixed_name` rows of `report` in report order as
/// `(offset, length)`.
fn undefined_prefixed_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_prefixed_name")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// Byte offset of `needle` in the fixture (ASCII: byte == UTF-16 offset).
fn at(source: &str, needle: &str) -> u32 {
    source
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture")) as u32
}

/// The `(offset, length)` of the leaf identifier of the prefixed spelling
/// `prefixed` (`M.z` → the `z` token): `undefined_prefixed_name` anchors at
/// the identifier node, never the whole prefixed spelling (the oracle's
/// `at(identifier)` report; the corpus row `25:11|7 'HashSet'` for
/// `c.HashSet`).
fn leaf_at(source: &str, prefixed: &str) -> (u32, u32) {
    let dot = prefixed.find('.').expect("prefixed spelling") + 1;
    (
        at(source, prefixed) + dot as u32,
        (prefixed.len() - dot) as u32,
    )
}

/// Diagnoses `main` importing `lib_source` under `prefix` (bare when
/// `None`) with `clauses` — `(hide, names)` pairs in written order, the
/// capture the engine diet walk owes. The names intern into the same
/// `StringTable` the graph carries (`StringId` is table-local, so clause
/// membership only compares inside one table). Mirrors the graph shape the
/// engine mounts: one dependency library, its declarations declared, one
/// import edge carrying the written combinators.
fn diagnose_with_combinators(
    main: &str,
    lib_source: &str,
    prefix: Option<&str>,
    clauses: &[(&bool, &[&str])],
) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("lib.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scanned_a = dartq_scanner::scan(
        lib_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    assert!(
        scanned_a.diagnostics.is_empty(),
        "{lib_source:?} scan diagnostics"
    );
    let scanned_m = dartq_scanner::scan(
        main,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    assert!(
        scanned_m.diagnostics.is_empty(),
        "{main:?} scan diagnostics"
    );
    let elements_a = collect(lib_source, &scanned_a.tokens, features, &strings);
    let elements_m = collect(main, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/lib.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, decl) in elements_a.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(lib_a, name, file_a);
        }
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(lib_m, name, file_m);
        }
    }
    let clauses = clauses
        .iter()
        .map(|&(hide, names)| {
            let names: Box<[StringId]> = names.iter().map(|n| strings.intern(n)).collect();
            if *hide {
                dartq_resolution::CombinatorClause::Hide(names)
            } else {
                dartq_resolution::CombinatorClause::Show(names)
            }
        })
        .collect::<Vec<_>>();
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: prefix.map(|p| strings.intern(p)),
            deferred: false,
            non_library: false,
            combinators: Combinators::new(clauses, &strings),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    diagnose(
        main,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
        &graph,
        &namespaces,
        &index,
    )
}

/// The recon's EXTRA core (combinators2_test, oracle row 15:9|4 only):
/// `show hide, show hide ugly` shows the keyword-spelled members `hide` and
/// `show` and hides `ugly` — the uses of `hide`/`show` resolve and only
/// `ugly` reports. With the correct capture the clause lists carry, the
/// lookup admits both pseudo-keyword names; the production EXTRA rows come
/// from the diet walk dropping them (module doc, residual 1).
#[test]
fn t3375_shown_keyword_named_members_resolve_hidden_misses() {
    let main = "import 'lib.dart' show hide, show hide ugly; \
                void f() { hide; show; ugly; }";
    let report = diagnose_with_combinators(
        main,
        "var show = 'show'; var hide = 'hide'; var ugly = 'ugly';",
        None,
        &[(&false, &["hide", "show"]), (&true, &["ugly"])],
    );
    assert_eq!(
        undefined_rows(&report),
        vec![(at(main, "ugly; }"), 4)],
        "{:?}",
        report.diagnostics
    );
}

/// The bare `hide` combinator admits everything else (the EXTRA shape's
/// subtraction twin): `visible` stays silent, the hidden `ugly` reports at
/// its own token.
#[test]
fn t3375_bare_import_hide_drops_only_hidden() {
    let main = "import 'lib.dart' hide ugly; void f() { visible; ugly; }";
    let report = diagnose_with_combinators(
        main,
        "var visible = 0; var ugly = 0;",
        None,
        &[(&true, &["ugly"])],
    );
    assert_eq!(
        undefined_rows(&report),
        vec![(at(main, "ugly; }"), 4)],
        "{:?}",
        report.diagnostics
    );
}

/// The prefixed `show` face (combinators_test `as M show e`, oracle row
/// `M.e` silent): only the shown name enters the prefix table; a live
/// prefix's unshown member reports `undefined_prefixed_name` over the
/// written `M.z` spelling, and a prefixed import contributes its prefix
/// spelling alone — the shown name is not in the bare scope.
#[test]
fn t3375_prefixed_show_admits_only_shown() {
    let main = "import 'lib.dart' as M show e; void f() { M.e; M.z; e; }";
    let report = diagnose_with_combinators(
        main,
        "var e = 0; var z = 0;",
        Some("M"),
        &[(&false, &["e"])],
    );
    assert_eq!(
        undefined_prefixed_rows(&report),
        vec![leaf_at(main, "M.z")],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        undefined_rows(&report),
        vec![(at(main, "e; }"), 1)],
        "{:?}",
        report.diagnostics
    );
}

/// The prefixed `hide` twin: the hidden member misses, the unhidden one
/// resolves.
#[test]
fn t3375_prefixed_hide_drops_only_hidden() {
    let main = "import 'lib.dart' as M hide z; void f() { M.e; M.z; }";
    let report =
        diagnose_with_combinators(main, "var e = 0; var z = 0;", Some("M"), &[(&true, &["z"])]);
    assert_eq!(
        undefined_prefixed_rows(&report),
        vec![leaf_at(main, "M.z")],
        "{:?}",
        report.diagnostics
    );
}

/// The brief's `p.Foo` class face with both combinator kinds on one prefix:
/// the shown class resolves, the hidden and the absent classes report
/// `undefined_prefixed_name`, and the bare scope carries the prefix spelling
/// alone (`Foo;` stays undefined — the oracle's unit-scope rule).
#[test]
fn t3375_prefix_show_hide_class_face() {
    let main = "import 'lib.dart' as p show Foo hide Bar; void f() { p.Foo; p.Bar; p.Baz; Foo; }";
    let report = diagnose_with_combinators(
        main,
        "class Foo {} class Bar {} class Baz {}",
        Some("p"),
        &[(&false, &["Foo"]), (&true, &["Bar"])],
    );
    assert_eq!(
        undefined_prefixed_rows(&report),
        vec![leaf_at(main, "p.Bar"), leaf_at(main, "p.Baz")],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        undefined_rows(&report),
        vec![(at(main, "Foo; }"), 3)],
        "{:?}",
        report.diagnostics
    );
}
