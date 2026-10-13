// Targets const_with_non_const: a const creation invoking a non-const
// constructor. Oracle message (nail-pinned): "The constructor being called
// isn't a const constructor."
class C {
  C();
}

const x = C();
