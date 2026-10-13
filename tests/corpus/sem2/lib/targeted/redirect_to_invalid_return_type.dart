// Targets redirect_to_invalid_return_type: a redirecting factory whose
// target's class is not a subtype of the enclosing class. Oracle message
// (nail-pinned): "The return type 'S' of the redirected constructor isn't
// a subtype of 'T'."
class S {}

class T {
  factory T() = S;
}
