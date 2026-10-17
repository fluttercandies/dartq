// unreachable_from_main — official rule faces
// (pkg/linter unreachable_from_main.dart:573-600 reporting, 696-735
// nameForError; test/rules/unreachable_from_main_test.dart:86-95,1483-1488,
// upstream Dart SDK linter test data): in a library that has an entry
// point (`main`), a public declaration with no reference path from any
// entry point reports at its name token ("Unreachable member '{0}' in an
// executable library."); a library with no entry point reports nothing at
// all, and a member nested in an unreachable container is suppressed
// behind the container.
//
// Expected rows: 2 — UNREACHABLE_FROM_MAIN keyed unreachable_from_main.dart,
// message "Unreachable member '{0}' in an executable library.":
//   `f` — public field of C, constructed from main but never read
//     (upstream test test_class_instanceField_unreachable)
//   `g` — public top-level function with no reference path from main
//     (upstream test test_topLevelFunction_unreachable)
void main() {
  C();
}

class C {
  int? f;
}

void g() {}
