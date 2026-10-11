// Targets unused_catch_clause: an 'on'-clause exception parameter that the
// catch body never uses.
void f() {
  try {
  } on FormatException catch (e) {
  }
}
