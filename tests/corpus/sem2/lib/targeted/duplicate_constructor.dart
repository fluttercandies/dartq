// Targets duplicate_constructor: two constructors with the same name in
// one class (the unnamed constructor twice). Oracle message (nail-pinned):
// "The unnamed constructor is already defined."
class D {
  D();
  D();
}
