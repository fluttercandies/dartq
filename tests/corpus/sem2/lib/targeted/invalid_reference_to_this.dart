// Targets invalid_reference_to_this: 'this' used as a constructor
// initializer target. Oracle message (nail-pinned): "Invalid reference to
// 'this' expression."
class C {
  C() : this = 0;
}
