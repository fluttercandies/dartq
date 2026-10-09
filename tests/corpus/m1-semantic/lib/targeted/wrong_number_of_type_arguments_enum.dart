// Targets wrong_number_of_type_arguments_enum: an enum constant declaration
// carries an explicit type argument list whose length differs from the
// enum's type parameter count.
enum E<T> {
  v<int, int>();

  const E();
}

void main() {
  print(E.v);
}
