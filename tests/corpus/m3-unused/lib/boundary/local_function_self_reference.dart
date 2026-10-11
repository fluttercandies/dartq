// Boundary self-reference: a local function invoking itself does not count
// as a reference (an executable's reference to itself is ignored), so an
// otherwise-unused local function still reports unused_element - official
// anchor: unused_element_test functionLocal_notUsed_referenceFromItself.
void f() {
  void g() {
    g();
  }
}
