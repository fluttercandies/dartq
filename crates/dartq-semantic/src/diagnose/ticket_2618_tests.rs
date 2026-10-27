//! The #2618 pins: resolved member-access reference edges — the diagnose
//! fold publishes one row per member send its own verdicts resolve, with
//! the target declaration (file + lookup spelling) and the intent-derived
//! kind. One pin per shape: plain and null-aware sends, cascades,
//! implicit-this and `this`/`super` accesses, the static container face,
//! extension members (qualified application, static access, and the bare
//! implicit application), getter reads, setter and field writes, compound
//! writes, and the tear-off. Named constructors carry no edge (the index's
//! declaration model has no constructor rows — the class-name face owns
//! the site) — pinned as the deliberate zero.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2570_tests`' shape) — returning the
/// report beside the intern table (the rows carry `StringId`s).
fn report(source: &str) -> (FileReport, StringTable) {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
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
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    (report, strings)
}

/// Guards the fixture: every member send resolved — the member families'
/// misses would silently drop the edge the pin expects. (`undefined_class`
/// is the type-use face: the rig carries no dart:core, so written `int` /
/// `Future` heads miss while the member resolution stands.)
fn assert_resolved(report: &FileReport) {
    const MEMBER_FAMILIES: [&str; 5] = [
        "undefined_method",
        "undefined_getter",
        "undefined_setter",
        "undefined_super_member",
        "undefined_extension_",
    ];
    let unresolved: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .filter(|name| MEMBER_FAMILIES.iter().any(|family| name.contains(family)))
        .collect();
    assert!(unresolved.is_empty(), "unresolved sends: {unresolved:?}");
}

/// The member-ref rows as `(offset, kind, target spelling)` triples,
/// offset-ascending — the deterministic projection the pins compare. The
/// row span is the MEMBER name token, so every expectation's offset lands
/// exactly on its spelling (asserted here, not trusted).
fn rows(source: &str, report: &FileReport, strings: &StringTable) -> Vec<(usize, &'static str, String)> {
    let mut rows: Vec<_> = report
        .member_refs
        .iter()
        .map(|row| {
            let spelling = strings.resolve(row.target_name);
            let written = spelling.strip_suffix('=').unwrap_or(spelling);
            let offset = row.range.offset as usize;
            assert!(
                source[offset..].starts_with(written),
                "the row at {offset} starts `{written}`: {}",
                &source[offset.min(source.len())..(offset + 16).min(source.len())]
            );
            (
                offset,
                match row.kind {
                    MemberRefKind::Read => "read",
                    MemberRefKind::Invoked => "invoked",
                    MemberRefKind::Written => "written",
                    MemberRefKind::ReadWritten => "read_written",
                },
                spelling.to_owned(),
            )
        })
        .collect();
    rows.sort_by_key(|&(offset, _, _)| offset);
    rows
}

/// The member-token offset: `needle`'s start plus `skip` (the receiver
/// spelling's length — the needle leads at the receiver, the row at the
/// member).
fn at(source: &str, needle: &str, skip: usize) -> usize {
    source.find(needle).unwrap_or_else(|| panic!("`{needle}` missing")) + skip
}

/// The send-shape sweep: every qualified and implicit-this member access
/// resolves to one edge with the intent-derived kind — invocation, tear-off,
/// getter/field read, plain and compound writes, cascades, null-aware
/// sends, `this`, and the static container face.
#[test]
fn member_refs_cover_the_send_shapes() {
    let source = "class Repo {\n\
                  int total = 0;\n\
                  int get limit => 7;\n\
                  set label(int v) {}\n\
                  void add(int v) { total = v; }\n\
                  void bump() { total += 1; }\n\
                  void reset() { this.add(0); add(1); }\n\
                  static Repo make() => Repo();\n\
                  }\n\
                  void use(Repo r, Repo? n) {\n\
                  r.add(2);\n\
                  n?.add(3);\n\
                  Repo s = Repo.make();\n\
                  s..add(4);\n\
                  s..add(5);\n\
                  use2(r.add);\n\
                  r.total;\n\
                  r.limit;\n\
                  r.label = 9;\n\
                  r.total += 10;\n\
                  }\n\
                  void use2(void Function(int) f) {}\n";
    let (report, strings) = report(source);
    assert_resolved(&report);
    // A cascade's LATER sections chain off the previous section's result —
    // a `void` return leaves the model's receiver Unknown, the same
    // classification the diagnostics trust — so the cascade pins use two
    // single-section cascades, each resolved through its own target.
    let expected: [(&str, usize, &str, &str); 14] = [
        ("total = v", 0, "written", "total"),
        ("total += 1", 0, "read_written", "total"),
        ("this.add", 5, "invoked", "add"),
        ("add(1)", 0, "invoked", "add"),
        ("r.add(2)", 2, "invoked", "add"),
        ("n?.add", 3, "invoked", "add"),
        ("Repo.make()", 5, "invoked", "make"),
        ("s..add(4)", 3, "invoked", "add"),
        ("s..add(5)", 3, "invoked", "add"),
        ("r.add)", 2, "invoked", "add"),
        ("r.total;", 2, "read", "total"),
        ("r.limit", 2, "read", "limit"),
        ("r.label = 9", 2, "written", "label="),
        ("r.total += 10", 2, "read_written", "total"),
    ];
    let wanted: Vec<(usize, &str, String)> = expected
        .iter()
        .map(|&(needle, skip, kind, target)| (at(source, needle, skip), kind, target.to_owned()))
        .collect();
    assert_eq!(rows(source, &report, &strings), wanted, "one edge per resolved member send");
}

/// `super.m` resolves onto the superclass declaration; a nullable
/// receiver's null-aware send keeps its edge.
#[test]
fn member_refs_cover_super_and_null_aware_sends() {
    let source = "class Base {\n\
                  void hi() {}\n\
                  }\n\
                  class Sub extends Base {\n\
                  void go() { super.hi(); }\n\
                  }\n\
                  void run(Sub? s) {\n\
                  s?.hi();\n\
                  }\n";
    let (report, strings) = report(source);
    assert_resolved(&report);
    let expected: [(&str, usize, &str, &str); 2] = [
        ("super.hi()", 6, "invoked", "hi"),
        ("s?.hi()", 3, "invoked", "hi"),
    ];
    let wanted: Vec<(usize, &str, String)> = expected
        .iter()
        .map(|&(needle, skip, kind, target)| (at(source, needle, skip), kind, target.to_owned()))
        .collect();
    assert_eq!(rows(source, &report, &strings), wanted);
}

/// Extension members carry edges through the application faces the model
/// can name: the qualified send over an `on`-typed receiver (the implicit
/// application inside `member_defined`) and the static access
/// `Ext.member()`. A BARE invoke that an extension resolves stays
/// `RefTarget::Opaque` at push time (the model's "cannot name the target"
/// answer for the extension pool) — no edge, pinned by absence here.
#[test]
fn member_refs_cover_extension_members() {
    let source = "class Host {}\n\
                  extension HostExt on Host {\n\
                  int boost() => 1;\n\
                  static int two() => 2;\n\
                  }\n\
                  class Client {\n\
                  Host h = Host();\n\
                  int viaReceiver() => h.boost();\n\
                  }\n\
                  class Bare implements Host {\n\
                  int viaBare() => boost();\n\
                  }\n\
                  int run() => HostExt.two();\n";
    let (report, strings) = report(source);
    assert_resolved(&report);
    let expected: [(&str, usize, &str, &str); 2] = [
        ("h.boost()", 2, "invoked", "boost"),
        ("HostExt.two()", 8, "invoked", "two"),
    ];
    let wanted: Vec<(usize, &str, String)> = expected
        .iter()
        .map(|&(needle, skip, kind, target)| (at(source, needle, skip), kind, target.to_owned()))
        .collect();
    assert_eq!(rows(source, &report, &strings), wanted);
}

/// A named constructor (`C.named()`) carries no member edge: the index's
/// declaration model has no constructor rows, so the class-name face (the
/// parser-level surface) owns the site — pinned as the deliberate zero.
#[test]
fn named_constructors_carry_no_member_edge() {
    let source = "class P {\n\
                  P.named();\n\
                  }\n\
                  void run() {\n\
                  P.named();\n\
                  }\n";
    let (report, strings) = report(source);
    assert_resolved(&report);
    assert!(
        rows(source, &report, &strings).is_empty(),
        "the named-constructor site rides the class-name face"
    );
}

/// The interface-typed receiver's edge targets the INTERFACE declaration —
/// the statically resolved one — never the implementing override (the
/// engine's override closure widens at the query boundary).
#[test]
fn interface_typed_receivers_target_the_interface_declaration() {
    let source = "abstract class R {\n\
                  Future<void> createTask();\n\
                  }\n\
                  class Impl implements R {\n\
                  Future<void> createTask() async {}\n\
                  }\n\
                  void f(R r) {\n\
                  r.createTask();\n\
                  }\n";
    let (report, strings) = report(source);
    assert_resolved(&report);
    let expected = vec![(at(source, "r.createTask()", 2), "invoked", "createTask".to_owned())];
    assert_eq!(rows(source, &report, &strings), expected);
}
