// Targets expected_case_or_default (syntax-codes.tsv:94): a plain
// statement where a 'case' clause or 'default' clause is required (after
// an existing case body).
// Provenance: hand-crafted against the pinned analyzer condition
// (crates/dartq-diagnostics/data/syntax-codes.tsv).
void switchWithTrailingStatement(Object v) {
  switch (v) {
    case 1:
      break;
    print(v);
  }
}
