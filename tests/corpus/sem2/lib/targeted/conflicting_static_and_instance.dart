// Targets conflicting_static_and_instance: a class defining a static
// member and an instance member with the same name. Oracle message
// (nail-pinned): "Class 'C' can't define static member 'm' and have
// instance member 'C.m' with the same name."
class C {
  static int m() => 0;

  int m2() => m();

  int m() => 1;
}
