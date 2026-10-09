// Targets static_operator: operators cannot be static.
class C {
  static int operator +(int x) => x + 1;
}
