// Targets unnecessary_cast_pattern: a cast pattern whose target already
// subsumes the matched type is dead (shared_type_analyzer).
void f(Object? o) {
  switch (o) {
    case _ as Object?:
      break;
  }
}
