// Targets null_aware_cascade_out_of_order: the '?..' cascade operator must be
// first in the cascade sequence.
void f(dynamic x) {
  x?..toString()?..toString();
}
