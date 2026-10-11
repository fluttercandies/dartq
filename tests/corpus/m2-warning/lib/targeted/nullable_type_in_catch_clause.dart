// Targets nullable_type_in_catch_clause: an 'on' clause cannot use a
// nullable type (_checkForNullableTypeInCatchClause).
void f() {
  try {
    throw 'x';
  } on Exception? catch (e) {
    print(e);
  }
}
