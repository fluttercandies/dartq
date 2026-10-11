// Targets the library-directive / URI family (analyzer ParserErrorCode,
// pinned snapshot syntax-codes.tsv; the analyzer sources are not vendored
// in .third).
// Targets non_identifier_library_name (TSV:246).
library 1;

// Targets non_string_literal_as_uri (TSV:248).
import dart:core;

// Targets invalid_literal_in_configuration (TSV:175): the configuration
// value literal can't contain interpolation.
import 'a.dart' if (dart.library.io == '$x') 'b.dart';

// Never-fires face (pre-review reclassification): missing_quote (TSV:219,
// "Expected quote (' or \").") has no emit site anywhere in the SDK — it
// exists only as a generated diagnostic definition
// (pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart) — and dartq has no
// emit site for it either. No shape can surface it; documented gap by
// construction, like the native family.
//
// The scanner-tier face of the quote family is unterminated_string_literal
// (TSV:288): an unterminated literal is closed at end of line, and both
// sides report the same scanner code (fasta UnterminatedString ->
// analyzer unterminated_string_literal; dartq scanner strings.rs emits the
// same recovery code).
import 'a.dart;

// The non-quoted default URI below plausibly yields
// non_string_literal_as_uri (TSV:248) from the oracle — the same face as
// the 'import dart:core;' shape above. Kept as overlap documentation; the
// pre-review label targeting missing_quote on this shape was wrong.
export 'c.dart' if (dart.library.html) d.dart;

void f() {}
