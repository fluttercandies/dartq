// Targets invalid_comment_reference (syntax-codes.tsv:162): a doc-comment
// reference must be a (possibly prefixed) identifier; this one contains an
// expression.
// Provenance: hand-crafted against the pinned analyzer condition
// (crates/dartq-diagnostics/data/syntax-codes.tsv); the analyzer comment
// scanner is not vendored in this repo (the SDK copy strips pkg/analyzer).
/// The reference [1 + 2] is not an identifier chain.
void documented() {}
