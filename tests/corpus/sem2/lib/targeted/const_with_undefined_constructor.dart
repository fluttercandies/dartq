// Targets const_with_undefined_constructor: a const creation naming a
// constructor the class does not have. Oracle message (nail-pinned): "The
// class 'C' doesn't have a constant constructor 'named'."
class C {
  const C();
}

const y = C.named();
