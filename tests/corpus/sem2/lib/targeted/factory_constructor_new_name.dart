// Targets factory_constructor_new_name: a factory constructor named 'new'.
// Oracle message (nail-pinned): "Factory constructors can't be named
// 'new'." (shape mirrored from the regression nail).
class K {
  factory new() => K();
}
