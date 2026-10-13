// Targets invalid_use_of_null_value: dereferencing an expression whose
// value is always null. Oracle message (nail-pinned): "An expression whose
// value is always 'null' can't be dereferenced."
void f() {
  null.toString();
}
