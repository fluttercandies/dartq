// package_prefixed_library_names — official rule faces
// (pkg/linter package_prefixed_library_names.dart): registered but
// permanently silent upstream — the visitor body is `return;` ("Project
// info is not being set", dart-lang/linter#3395) — so parity is the
// no-op: the oracle emits nothing for this rule and a live dartq
// reimplementation would fire where the oracle stays silent. The fixture
// pins that silence under the corpus options: zero rows on both sides,
// named library or not.
// Expected rows: 0 — PACKAGE_PREFIXED_LIBRARY_NAMES keyed
// package_prefixed_library_names.dart: the upstream no-op face emits
// nothing for the dotted library name, and dartq mirrors the silence.
library foo.bar;

class A {}
