// Targets list_element_type_not_assignable: a typed list literal whose
// element type is not assignable to the element type.
void main() {
  var l = <int>['a'];
  print(l);
}
