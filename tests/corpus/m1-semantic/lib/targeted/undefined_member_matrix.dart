// Targets the member-access undefined family on one nominal receiver:
// undefined_getter, undefined_setter, undefined_method, and
// undefined_named_parameter (instance member lookup on a user class; the
// method name avoids the Object.noSuchMethod collision noted in
// undefined_method.dart), plus undefined_operator (binary operator on a
// class that declares none). Trigger conditions: the pinned
// resolution trigger table (lookup failures against the resolved import scope). Hand-crafted from the code semantics; no
// official test file.
class C {
  int m(int a) => a;
}

void use(C c) {
  print(c.noSuchGetter);
  c.noSuchSetter = 1;
  c.noSuchMemberHere();
  c.m(b: 1);
  print(c + c);
}
