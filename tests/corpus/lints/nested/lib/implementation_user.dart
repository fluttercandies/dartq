// implementation_imports — the canonical face: an import of another
// package's lib/src implementation path. This file doubles as the child
// include-chain control: the row only exists when the child options'
// include chain resolved (the rule comes from the parent's
// package:lints include), so a broken child chain turns this expect-row
// into a vacuous silence on both sides.
//
// The child pubspec declares lint_corpus as a path dependency, so
// depend_on_referenced_packages stays silent and the only row is the
// implementation-path one (both sides; the file sits under the child's
// lib/ and the target is another package's src/).
// Expected rows: 1 — IMPLEMENTATION_IMPORTS keyed
// nested/lib/implementation_user.dart.
import 'package:lint_corpus/src/impl.dart';

void implementationUser() {
  implToken();
}
