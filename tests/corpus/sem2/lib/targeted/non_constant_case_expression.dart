// Targets non_constant_case_expression: a legacy (pre-patterns) case
// expression that is not constant; the per-file language downgrade matches
// the regression nail. Verbatim mirror of the nail source (co-fires
// expected_token, syntax tier, and undefined_identifier, m1).
// @dart=2.19
main() {
  switch(a) {
    // Parse as conditional (with missing colon for the case).
    case x?[4]:{{{2}}}; // as case (x?[4]:{{{2}}}) [missing :]
  }
}
