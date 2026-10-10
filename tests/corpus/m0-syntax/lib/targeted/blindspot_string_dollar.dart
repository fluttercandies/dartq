// Targets unexpected_dollar_in_string (syntax-codes.tsv:282): a '$' in a
// string must be followed by an identifier or '{'. Scanner emit site:
// pkg/_fe_analyzer_shared/lib/src/scanner/abstract_scanner.dart:2298
// (diag.unexpectedDollarInString).
var dollarThenDigit = 'a $5 b';
