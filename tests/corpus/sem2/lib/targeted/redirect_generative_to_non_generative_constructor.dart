// Targets redirect_generative_to_non_generative_constructor: a generative
// constructor redirecting to a factory constructor. Oracle message
// (nail-pinned): "Generative constructors can't redirect to a factory
// constructor."
class W {
  factory W.n() => W();

  W() : this.n();
}
