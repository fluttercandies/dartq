// Targets wrong_number_of_type_arguments: a generic class type annotation is
// given more type arguments than it declares (plain variant).
class C<T> {}

void main() {
  late C<int, String> x;
}
