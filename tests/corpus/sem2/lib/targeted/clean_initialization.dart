// Clean face for the initialization family (const_not_initialized,
// final_not_initialized, not_initialized_non_nullable_variable,
// not_initialized_non_nullable_instance_field, and the dartq-side renames
// const_field_without_initializer / final_field_without_initializer):
// every variable initialized or definitely assigned; both sides must stay
// silent.
const topConst = 0;
final topFinal = 0;

class C {
  final int f;
  static const s = 0;
  late int lateField;

  C(this.f);
}

void main() {
  int local;
  local = 1;
  print(local);
  print(topConst + topFinal + C.s);
}
