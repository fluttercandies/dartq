// Targets invalid_factory_name_not_a_class: a factory constructor whose
// name is not the enclosing class. Oracle message (nail-pinned): "The name
// of a factory constructor must be the same as the name of the immediately
// enclosing class."
class G {
  factory H() => G();
}
