// Targets non_type_in_catch_clause: an on-catch clause naming a non-type.
// Oracle message (nail-pinned): "The name 'Foo' isn't a type and can't be
// used in an on-catch clause."
void f() {
  try {} on Foo {}
}
