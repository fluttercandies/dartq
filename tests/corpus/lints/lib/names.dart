// names family — audit r1 near-blind rows (corpora carry conforming
// names). One violation per naming rule, each on its own line.
// Expected rows: 3, all keyed names.dart — CAMEL_CASE_TYPES
// (lower_case_class), CONSTANT_IDENTIFIER_NAMES (MAX_LEVEL),
// NON_CONSTANT_IDENTIFIER_NAMES (MisnamedFunction).
class lower_case_class {}

const int MAX_LEVEL = 9;

void MisnamedFunction() {}
