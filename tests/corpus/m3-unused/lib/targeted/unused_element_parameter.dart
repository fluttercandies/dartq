// Targets unused_element_parameter: an optional parameter of a private
// method that no invocation ever passes — _m is invoked (so the method
// itself is used), but a value for y is never given.
class C {
  void _m([int? y]) {}

  void invoke() {
    _m();
  }
}
