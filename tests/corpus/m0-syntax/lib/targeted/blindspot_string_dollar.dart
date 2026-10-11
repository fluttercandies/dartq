// Never-fires face (TSV:282): unexpected_dollar_in_string never reaches
// analyzer output — the error-token translation remaps it to
// missing_identifier before reporting (pkg/analyzer/lib/src/dart/scanner/
// translate_error_token.dart:105-106, unexpectedDollarInString ->
// missingIdentifier). dartq mirrors the remap in its scanner
// (crates/dartq-scanner/src/lexer/strings.rs: "official
// unexpectedDollarInString -> analyzer missing_identifier"), so the
// underlying face — a '$' in a string must be followed by an identifier
// or '{' (fasta emit site pkg/_fe_analyzer_shared/lib/src/scanner/
// abstract_scanner.dart:2298, diag.unexpectedDollarInString) — is
// exercised on both sides via missing_identifier (TSV:209). Like the
// native family, the code itself stays in the never-fires set: a
// documented gap by construction, not a divergence.
var dollarThenDigit = 'a $5 b';
