//! The #3608 pins — the library close's member credit is element-level.
//!
//! The per-file pass decides every unused verdict against the ELEMENT each
//! use bound (`_isUsedMember`'s `usedElements.members` ∪ `elements`,
//! upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart:977-992`;
//! `_isReadMember`'s `readMembers`, :880-908). [`Engine::analyze`] then
//! re-decides the staged rows once more at the library close, against the
//! merged per-library pool — and at name granularity that re-decision
//! releases a row as soon as a *different* declaration shares the spelling
//! (`A._m1` spared by a use of `B._m1`), the under-report direction these
//! pins lock down end-to-end.
//!
//! Oracle probe (`dart analyze --format=machine`, SDK 3.13.7, package
//! language version 3.13) over the `sibling` fixture reports
//! `UNUSED_ELEMENT` at `A._m1` and `UNUSED_FIELD` at `A._f`; `B`'s
//! same-spelled members stay silent because the uses resolved onto them.

mod common;

use common::Fixture;
use dartq_diagnostics::Diagnostic;
use dartq_engine::CancellationToken;
use dartq_source::FileId;

/// The offsets of `code`'s rows in `file`, in diagnostic order.
fn offsets(diagnostics: &[Diagnostic], file: FileId, code: &str) -> Vec<u32> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.file == file && diagnostic.code.name == code)
        .map(|diagnostic| diagnostic.range.offset)
        .collect()
}

/// Analyzes `libraries` (written to the throwaway workspace) plus `main`,
/// returning the diagnostics and `lib/main.dart`'s id.
fn analyze(libraries: &[(&str, &str)], main: &str) -> (Vec<Diagnostic>, FileId) {
    let fixture = Fixture::new(libraries);
    let mut engine = fixture.engine();
    for (path, content) in libraries {
        engine.set_content(&fixture.path(path), content.as_bytes());
    }
    let file = engine.set_content(&fixture.path("lib/main.dart"), main.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    (diagnostics, file)
}

/// The declaration-name offset of the first `needle` in `source` (the
/// fixtures are ASCII, so byte and UTF-16 offsets coincide).
fn name_offset(source: &str, needle: &str) -> u32 {
    u32::try_from(source.find(needle).expect("fixture holds the declaration")).expect("offset")
}

/// `A` and `B` declare same-spelled members; `B`'s are used, `A`'s are not.
const SIBLING: &str = "\
class A {
  void _m1() {}
  int _f = 0;
}

class B {
  void start() => _m1();
  void _m1() {}
  int _f = 0;
  int get peek => _f;
}
";

/// A use of `B._m1` must not spare `A._m1` (`_isUsedMember`'s element
/// identity): exactly one `unused_element` row, anchored on `A`'s
/// declaration and not `B`'s. The name-level close releases both.
#[test]
fn member_use_does_not_spare_same_named_sibling() {
    let (diagnostics, file) = analyze(&[], SIBLING);
    let rows = offsets(&diagnostics, file, "unused_element");
    let expected = name_offset(SIBLING, "void _m1() {}") + u32::try_from("void ".len()).unwrap();
    assert_eq!(
        rows,
        vec![expected],
        "`A._m1` alone reports (B's same-spelled method is used): {rows:?}"
    );
}

/// The `unused_field` twin (`_isReadMember`'s `readMembers` element set): a
/// read of `B._f` spares the field it resolved, never `A._f`.
#[test]
fn field_read_does_not_spare_same_named_sibling() {
    let (diagnostics, file) = analyze(&[], SIBLING);
    let rows = offsets(&diagnostics, file, "unused_field");
    let expected = name_offset(SIBLING, "int _f = 0;") + u32::try_from("int ".len()).unwrap();
    assert_eq!(
        rows,
        vec![expected],
        "`A._f` alone reports (B's same-spelled field is read): {rows:?}"
    );
}

/// The release direction the element gate must keep: a use in a sibling
/// PART file resolves onto the declaring element, so the library close
/// releases both rows (`_isUsedMember`/`_isReadMember` fold one library-wide
/// element set).
#[test]
fn part_file_use_releases_the_declaring_element() {
    const MAIN: &str = "\
part 'p.dart';

class E {
  void _u() {}
  int _n = 0;
}
";
    const PART: &str = "\
part of 'main.dart';

void touch(E e) {
  e._u();
  print(e._n);
}
";
    let (diagnostics, file) = analyze(&[("lib/p.dart", PART)], MAIN);
    let rows = offsets(&diagnostics, file, "unused_element");
    assert!(
        rows.is_empty(),
        "the part's resolved uses release the element: {rows:?}"
    );
    let rows = offsets(&diagnostics, file, "unused_field");
    assert!(
        rows.is_empty(),
        "the part's resolved read releases the field: {rows:?}"
    );
}
