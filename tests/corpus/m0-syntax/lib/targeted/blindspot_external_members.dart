// Targets external_getter_with_body (syntax-codes.tsv:121),
// external_setter_with_body (TSV:125), external_operator_with_body
// (TSV:124): external accessors and operators can't have a body.
// Provenance: hand-crafted against the pinned analyzer condition
// (crates/dartq-diagnostics/data/syntax-codes.tsv); the analyzer sources
// are not vendored in .third (stripped pkg/analyzer).
class C {
  external int get x => 1;
  external set x(int v) {}
  external C operator +(C other) {
    return this;
  }
}
