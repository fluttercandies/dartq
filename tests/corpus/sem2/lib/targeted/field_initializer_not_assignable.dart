// Targets field_initializer_not_assignable: a field initializer whose type
// is not assignable to the field. Oracle message (nail-pinned): "The
// initializer type 'String' can't be assigned to the field type 'int'."
class C {
  int x;

  C(String s) : x = s;
}
