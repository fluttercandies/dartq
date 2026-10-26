//! The #2594 family pins: diagnose passes that walk source bytes must
//! convert their UTF-16 range anchors through `Engine::byte_of` before
//! slicing — a raw `&source[range.offset as usize..]` lands mid-character
//! (or silently desyncs) on any source with non-ASCII text upstream, the
//! live cockpit repro's `end byte index 9424 is not a char boundary; it is
//! inside '۲'` (`assignability.rs`, the typed-list element walk over
//! flutter's `time_picker_test.dart`). Every fixture here carries wide
//! characters (BMP Persian/Arabic and astral emoji) upstream of the face
//! under pin; the rigs stay self-contained so the shared `tests` rig is
//! untouched.

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports.
fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn the_code<'r>(report: &'r FileReport, name: &str) -> &'r dartq_diagnostics::Diagnostic {
    report
        .diagnostics
        .iter()
        .find(|d| d.code.name == name)
        .unwrap_or_else(|| panic!("{name} not reported: {:?}", report.diagnostics))
}

/// The UTF-16 offset of `needle`'s first byte — diagnostics ranges are
/// UTF-16 code units, not byte offsets.
fn u16_at(source: &str, needle: &str) -> u32 {
    let at = source
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in source"));
    u32::try_from(source[..at].chars().map(char::len_utf16).sum::<usize>())
        .expect("offset fits u32")
}

/// The source text a UTF-16 diagnostic range covers (its byte-domain
/// slice): the round-trip every consumer of a row range performs.
fn text_at_u16(source: &str, offset: u32, length: u32) -> &str {
    let mut units = 0u32;
    let mut start = None;
    let mut end = source.len();
    for (byte, ch) in source.char_indices() {
        if start.is_none() && units >= offset {
            start = Some(byte);
        }
        units += u32::try_from(ch.len_utf16()).expect("fits u32");
        if start.is_some() && units >= offset + length {
            end = byte + ch.len_utf8();
            break;
        }
    }
    &source[start.unwrap_or(source.len())..end]
}

/// Pin 1 — the live panic's exact shape
/// (`check_list_literal_elements`, assignability.rs): a typed list
/// literal whose wide-character elements make the closing `]`'s UTF-16
/// offset land inside `۲` (`<String>['۰', '۱۲۳']`: the `]` sits at byte
/// 63, its UTF-16 offset 59 is the second byte of `۱۲۳`'s `۲`). The
/// pre-fix element walk sliced `&source[start..stop]` at that raw offset
/// and panicked; the conversion makes the walk total, and the elements —
/// all strings — stay assignable.
#[test]
fn typed_list_literal_wide_elements_no_panic() {
    let report = unit_report(
        "class String {}\nvoid main() {\n  var l = <String>['۰', '۱۲۳'];\n  print(l);\n}\n",
    );
    assert_eq!(
        count_code(&report, "list_element_type_not_assignable"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// Pin 1b — the same walk stays CORRECT, not just total: with wide
/// characters upstream of a failing element, the element check still
/// fires (the pre-fix mixed-domain type-use filter silently skipped the
/// head) and anchors on the failing element's own row.
#[test]
fn typed_list_literal_element_check_fires_with_wide_prefix() {
    let source = "class String {}\nclass int {}\nvoid main() {\n  var s = '۲۳۲۳';\n  var l = <int>['a'];\n  print(l + s);\n}\n";
    let report = unit_report(source);
    let d = the_code(&report, "list_element_type_not_assignable");
    assert_eq!(
        text_at_u16(source, d.range.offset, d.range.length),
        "'a'",
        "the element row's UTF-16 range must slice the failing element"
    );
}

/// Pin 2 — `trimmed_span` (engine_core.rs): the `== null` comparison's
/// operator anchor over a never-null operand is re-derived from the
/// operand/null row spans through the byte domain; with wide characters
/// upstream, the pre-fix raw slice desynced. The reported span opens at
/// the trimmed `==` operator and runs through `null` — exactly, in UTF-16
/// coordinates.
#[test]
fn null_comparison_anchor_exact_with_wide_prefix() {
    let source = "class int {}\nvoid main() {\n  var s = '۲۳۲۳۲۳';\n  int x = 1;\n  if (x == null) {\n    print(s);\n  }\n}\n";
    let report = unit_report(source);
    let d = the_code(&report, "unnecessary_null_comparison");
    assert_eq!(d.range.offset, u16_at(source, "=="));
    assert_eq!(
        text_at_u16(source, d.range.offset, d.range.length),
        "== null"
    );
}

/// Pin 3 — the flow-fact name slice (diagnose.rs, `UnassignedRead`): a
/// definitely-unassigned `late` read reports with the variable's name
/// sliced from source. Eight wide characters before the read make the
/// read's UTF-16 start offset land mid-`۲` in the byte domain (the read
/// sits at byte 56, its UTF-16 offset 48 is a `۲` continuation byte) —
/// the pre-fix raw slice panicked; the face itself must still report.
#[test]
fn unassigned_late_read_name_with_wide_prefix() {
    let report =
        unit_report("class int {}\nvoid main() {\n  late int x;\n  print('۲۲۲۲۲۲۲۲', x);\n}\n");
    assert_eq!(
        count_code(&report, "definitely_unassigned_late_local_variable"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// The full-family torture fixture: BMP Persian/Arabic (RTL) and astral
/// emoji through every face the #2594 sweep hardened — typed
/// list/set/map literals (`assignability.rs`), the `== null` operator
/// anchor and switch-member body trims (`engine_core.rs`/`listener.rs`),
/// const-creation selector re-reads (`const_eval_faces.rs`), the
/// annotation argument walk (`note_metadata_args`), factory heads
/// (`end_factory`), the with-clause name slice (`finalize_mixin_uses`),
/// and the unassigned-late read — one pipeline pass, no panic, and the
/// engineered faces answer.
#[test]
fn multibyte_source_torture_flows_all_faces() {
    let source = concat!(
        "// دیدار مرحبا 🎯 RTL comment\n",
        "class String {}\n",
        "class int {}\n",
        "class Mix {}\n",
        "class P { const P(); }\n",
        "class C { const C(); }\n",
        "@P()\n",
        "class W with Mix {}\n",
        "const c = C();\n",
        "void tag(@P(parameterDefined: true) int b) { print(b); }\n",
        "void main() {\n",
        "  var fa = 'فارسی ۲۳۴ مرحبا';\n",
        "  var emoji = '🎯🚀 ünïcode';\n",
        "  var l = <String>['مرحبا', '۲۳', '🎯'];\n",
        "  var m = <String, int>{'۲': 1, 'مرحبا': 2};\n",
        "  var s = <String>{'۲۳', emoji};\n",
        "  var l2 = <int>['a'];\n",
        "  int x = 1;\n",
        "  if (x == null) {\n",
        "    print(fa);\n",
        "  }\n",
        "  switch (x) {\n",
        "    case 1:\n",
        "      print('۲۳۲۳ داخل الحالة');\n",
        "  }\n",
        "  late int q;\n",
        "  print('۲۲۲۲۲۲۲۲', q);\n",
        "  print(l + m.toString() + s.toString() + l2.toString() + c.toString());\n",
        "  tag(1);\n",
        "}\n"
    );
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "list_element_type_not_assignable"),
        1,
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        count_code(&report, "unnecessary_null_comparison"),
        1,
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        count_code(&report, "class_used_as_mixin"),
        1,
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        count_code(&report, "definitely_unassigned_late_local_variable"),
        1,
        "{:?}",
        report.diagnostics
    );
}
