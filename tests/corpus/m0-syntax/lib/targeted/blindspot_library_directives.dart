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

// Targets missing_quote (TSV:219): the configuration URI needs a quote.
// The oracle may report non_string_literal_as_uri for the same shape
// instead; both are gate codes.
export 'c.dart' if (dart.library.html) d.dart;

void f() {}
