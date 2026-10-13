// Clean face for the const-evaluation family (const_with_non_const,
// const_with_non_constant_argument, const_with_undefined_constructor,
// const_instance_field, non_constant_record_field, const_eval_type_string):
// every const creation over a const constructor with constant arguments;
// both sides must stay silent.
class C {
  final int x;
  const C(this.x);

  static const named = C(1);
}

const z = C(2);

const rec = (1, 2);

void main() {
  print(z.x + rec.$1 + C.named.x);
}
