// Intentional diagnostics: undefined_identifier. Two unrelated statements
// each reference one unresolved name; the expect-comments pin one code per
// error line (see README.md for the convention).
void reconcile() {
  // expect: undefined_identifier
  final total = unreportedCount + 1;
  // expect: undefined_identifier
  ledgerOpen = true;
  print(total);
}
